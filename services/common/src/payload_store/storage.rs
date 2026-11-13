// SPDX-License-Identifier: CC0-1.0

use super::types::*;
use bytes::Bytes;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

pub(crate) enum Storage {
    Memory {
        stages: HashMap<String, Vec<u8>>,
        blobs: HashMap<String, Bytes>,
        used: u64,
    },
    Volume {
        directory: PathBuf,
        used: u64,
        retained_files: u64,
        uncertain: bool,
    },
}
pub(crate) enum ReadSource {
    Memory(Bytes),
    File(File),
}
fn io<T>(result: std::io::Result<T>) -> Result<T, StoreError> {
    result.map_err(|_| StoreError::Io)
}
fn sync_dir(path: &Path) -> Result<(), StoreError> {
    io(File::open(path)?.sync_all())
}
fn regular(path: &Path) -> Result<File, StoreError> {
    let before = fs::symlink_metadata(path).map_err(|_| StoreError::Integrity)?;
    if !before.is_file() || before.file_type().is_symlink() {
        return Err(StoreError::Integrity);
    }
    let file = File::open(path).map_err(|_| StoreError::Integrity)?;
    let opened = file.metadata().map_err(|_| StoreError::Integrity)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.dev() != opened.dev() || before.ino() != opened.ino() {
            return Err(StoreError::Integrity);
        }
    }
    Ok(file)
}
fn create(path: &Path) -> Result<File, StoreError> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true).read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    io(options.open(path))
}
fn directory(path: &Path) -> Result<(), StoreError> {
    let meta = io(fs::symlink_metadata(path))?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err(StoreError::Invalid("owned volume directory"));
    }
    Ok(())
}
pub(crate) fn digest_reader(
    reader: &mut impl Read,
    limit: u64,
) -> Result<(u64, Sha256Digest), StoreError> {
    let mut hash = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = io(reader.read(&mut buffer))?;
        if count == 0 {
            break;
        }
        total = total
            .checked_add(count as u64)
            .ok_or(StoreError::QuotaExceeded)?;
        if total > limit {
            return Err(StoreError::Integrity);
        }
        hash.update(&buffer[..count]);
    }
    Ok((total, Sha256Digest(hash.finalize().into())))
}
impl Storage {
    pub fn memory() -> Self {
        Self::Memory {
            stages: HashMap::new(),
            blobs: HashMap::new(),
            used: 0,
        }
    }
    /// Only creates one store-owned child. Existing roots are never reset or recursively traversed.
    pub fn volume(
        root: &Path,
        scope: &Scope,
        max_files: usize,
    ) -> Result<(Self, bool), StoreError> {
        directory(root)?;
        let encoded = serde_json::to_vec(&[&scope.namespace, &scope.generation])
            .map_err(|_| StoreError::Catalog)?;
        let name = Sha256Digest(Sha256::digest(encoded).into()).as_hex();
        let path = root.join(format!("payload-v1-{name}"));
        let fresh = match fs::create_dir(&path) {
            Ok(()) => true,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => false,
            Err(_) => return Err(StoreError::Io),
        };
        directory(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if fresh {
                io(fs::set_permissions(
                    &path,
                    fs::Permissions::from_mode(0o700),
                ))?;
            }
        }
        let envelope = serde_json::json!({"format":"aws-emulator.state","service":"payload-store","namespace":scope.namespace,"generation":scope.generation,"version":1,"kind":"store-owner"});
        if fresh {
            let mut file = create(&path.join("identity.json"))?;
            io(file.write_all(&serde_json::to_vec(&envelope).map_err(|_| StoreError::Catalog)?))?;
            io(file.sync_all())?;
            for part in ["stages", "blobs", "temporary"] {
                io(fs::create_dir(path.join(part)))?;
            }
            sync_dir(&path)?;
            sync_dir(root)?;
        } else {
            let mut file = regular(&path.join("identity.json"))?;
            let mut bytes = Vec::new();
            io(Read::by_ref(&mut file).take(4097).read_to_end(&mut bytes))?;
            if bytes.len() > 4096
                || serde_json::from_slice::<serde_json::Value>(&bytes)
                    .ok()
                    .as_ref()
                    != Some(&envelope)
            {
                return Err(StoreError::NamespaceMismatch);
            }
        }
        for part in ["stages", "blobs", "temporary"] {
            directory(&path.join(part))?;
        }
        let mut used = 0u64;
        let mut retained_files = 0u64;
        // Metadata-only inventory, including unfinished/unknown files. Never adopt or delete them.
        for part in ["stages", "blobs", "temporary"] {
            for entry in io(fs::read_dir(path.join(part)))? {
                let entry = io(entry)?;
                let metadata = io(fs::symlink_metadata(entry.path()))?;
                if !metadata.is_file() || metadata.file_type().is_symlink() {
                    return Err(StoreError::Integrity);
                }
                retained_files = retained_files
                    .checked_add(1)
                    .ok_or(StoreError::QuotaExceeded)?;
                if retained_files > max_files as u64 {
                    return Err(StoreError::QuotaExceeded);
                }
                used = used
                    .checked_add(metadata.len())
                    .ok_or(StoreError::QuotaExceeded)?;
            }
        }
        Ok((
            Self::Volume {
                directory: path,
                used,
                retained_files,
                uncertain: false,
            },
            fresh,
        ))
    }
    pub fn catalog_path(&self) -> Option<PathBuf> {
        match self {
            Self::Volume { directory, .. } => Some(directory.join("catalog.sqlite3")),
            _ => None,
        }
    }
    pub fn sync_directory(&self) -> Result<(), StoreError> {
        if let Self::Volume { directory, .. } = self {
            sync_dir(directory)?;
        }
        Ok(())
    }
    pub fn accounting_uncertain(&self) -> bool {
        matches!(
            self,
            Self::Volume {
                uncertain: true,
                ..
            }
        )
    }
    pub fn used(&self) -> u64 {
        if self.accounting_uncertain() {
            return u64::MAX;
        }
        match self {
            Self::Memory { used, .. } | Self::Volume { used, .. } => *used,
        }
    }
    pub fn retained_files(&self) -> u64 {
        match self {
            Self::Volume { retained_files, .. } => *retained_files,
            Self::Memory { stages, blobs, .. } => (stages.len() + blobs.len()) as u64,
        }
    }
    fn stage_path(&self, id: &str) -> Result<PathBuf, StoreError> {
        match self {
            Self::Volume { directory, .. } => Ok(directory.join("stages").join(id)),
            _ => Err(StoreError::Catalog),
        }
    }
    fn blob_path(&self, digest: &str) -> Result<PathBuf, StoreError> {
        match self {
            Self::Volume { directory, .. } => Ok(directory.join("blobs").join(digest)),
            _ => Err(StoreError::Catalog),
        }
    }
    pub fn create_stage(&mut self, id: &str, maximum_files: usize) -> Result<(), StoreError> {
        if self
            .retained_files()
            .checked_add(1)
            .is_none_or(|n| n > maximum_files as u64)
        {
            return Err(StoreError::QuotaExceeded);
        }
        match self {
            Self::Memory { stages, .. } => {
                if stages.contains_key(id) {
                    return Err(StoreError::CheckpointConflict);
                }
                stages.insert(id.into(), Vec::new());
            }
            Self::Volume { .. } => {
                let path = self.stage_path(id)?;
                let file = create(&path)?;
                if let Self::Volume { retained_files, .. } = self {
                    *retained_files += 1;
                }
                io(file.sync_all())?;
                sync_dir(path.parent().ok_or(StoreError::Catalog)?)?;
            }
        }
        Ok(())
    }
    pub fn append(&mut self, id: &str, offset: u64, bytes: &[u8]) -> Result<(), StoreError> {
        match self {
            Self::Memory { stages, used, .. } => {
                let data = stages.get_mut(id).ok_or(StoreError::NotFound)?;
                if data.len() as u64 != offset {
                    return Err(StoreError::Integrity);
                }
                data.extend_from_slice(bytes);
                *used += bytes.len() as u64;
            }
            Self::Volume { .. } => {
                let path = self.stage_path(id)?;
                let observed = regular(&path)?
                    .metadata()
                    .map_err(|_| StoreError::Integrity)?
                    .len();
                if observed != offset {
                    return Err(StoreError::Integrity);
                }
                let mut file = io(OpenOptions::new().write(true).open(&path))?;
                io(file.seek(SeekFrom::Start(offset)))?;
                let result = file.write_all(bytes);
                let after = match file.metadata() {
                    Ok(metadata) => metadata.len(),
                    Err(_) => {
                        if let Self::Volume { uncertain, .. } = self {
                            *uncertain = true;
                        }
                        return Err(StoreError::Io);
                    }
                };
                if let Self::Volume { used, .. } = self {
                    *used = used.saturating_add(after.saturating_sub(observed));
                }
                io(result)?;
            }
        }
        Ok(())
    }
    pub fn flush_stage(&self, id: &str) -> Result<(), StoreError> {
        if let Self::Volume { .. } = self {
            io(regular(&self.stage_path(id)?)?.sync_all())?;
        }
        Ok(())
    }
    pub fn hash_stage(&self, id: &str, maximum: u64) -> Result<(u64, Sha256Digest), StoreError> {
        match self {
            Self::Memory { stages, .. } => {
                let bytes = stages.get(id).ok_or(StoreError::NotFound)?;
                if bytes.len() as u64 > maximum {
                    return Err(StoreError::Integrity);
                }
                Ok((
                    bytes.len() as u64,
                    Sha256Digest(Sha256::digest(bytes).into()),
                ))
            }
            Self::Volume { .. } => digest_reader(&mut regular(&self.stage_path(id)?)?, maximum),
        }
    }
    pub fn verify_checkpoint(
        &self,
        id: &str,
        length: u64,
        expected: Sha256Digest,
    ) -> Result<(), StoreError> {
        let actual = match self {
            Self::Memory { stages, .. } => {
                let data = stages.get(id).ok_or(StoreError::Integrity)?;
                if (data.len() as u64) < length {
                    return Err(StoreError::Integrity);
                }
                (
                    length,
                    Sha256Digest(Sha256::digest(&data[..length as usize]).into()),
                )
            }
            Self::Volume { .. } => {
                let mut input = regular(&self.stage_path(id)?)?.take(length);
                digest_reader(&mut input, length)?
            }
        };
        if actual != (length, expected) {
            return Err(StoreError::Integrity);
        }
        Ok(())
    }
    pub fn resume_stage(
        &mut self,
        id: &str,
        length: u64,
        expected: Sha256Digest,
    ) -> Result<Sha256, StoreError> {
        let mut hash = Sha256::new();
        match self {
            Self::Memory { stages, used, .. } => {
                let data = stages.get_mut(id).ok_or(StoreError::Integrity)?;
                if (data.len() as u64) < length {
                    return Err(StoreError::Integrity);
                }
                hash.update(&data[..length as usize]);
                if Sha256Digest(hash.clone().finalize().into()) != expected {
                    return Err(StoreError::Integrity);
                }
                *used -= data.len() as u64 - length;
                data.truncate(length as usize);
            }
            Self::Volume { .. } => {
                let path = self.stage_path(id)?;
                let mut reader = regular(&path)?;
                let before = io(reader.metadata())?.len();
                if before < length {
                    return Err(StoreError::Integrity);
                }
                let mut limited = Read::by_ref(&mut reader).take(length);
                let mut buffer = [0u8; 64 * 1024];
                let mut read = 0u64;
                loop {
                    let count = io(limited.read(&mut buffer))?;
                    if count == 0 {
                        break;
                    }
                    hash.update(&buffer[..count]);
                    read += count as u64;
                }
                if read != length || Sha256Digest(hash.clone().finalize().into()) != expected {
                    return Err(StoreError::Integrity);
                }
                let file = io(OpenOptions::new().write(true).open(&path))?;
                io(file.set_len(length))?;
                io(file.sync_all())?;
                if let Self::Volume { used, .. } = self {
                    *used -= before - length;
                }
            }
        }
        Ok(hash)
    }
    /// Keep the resumable stage separate: no hard-link alias can later mutate published content.
    pub fn install(
        &mut self,
        id: &str,
        digest: Sha256Digest,
        length: u64,
        maximum: u64,
        maximum_files: usize,
    ) -> Result<(), StoreError> {
        let key = digest.as_hex();
        match self {
            Self::Memory {
                stages,
                blobs,
                used,
            } => {
                if let Some(existing) = blobs.get(&key) {
                    if existing.len() as u64 != length {
                        return Err(StoreError::Integrity);
                    }
                    return Ok(());
                }
                if used.checked_add(length).is_none_or(|n| n > maximum) {
                    return Err(StoreError::QuotaExceeded);
                }
                if stages
                    .len()
                    .checked_add(blobs.len())
                    .and_then(|n| n.checked_add(1))
                    .is_none_or(|n| n > maximum_files)
                {
                    return Err(StoreError::QuotaExceeded);
                }
                let data = stages.get(id).ok_or(StoreError::Integrity)?;
                blobs.insert(key, Bytes::copy_from_slice(data));
                *used += length;
            }
            Self::Volume { .. } => {
                let final_path = self.blob_path(&key)?;
                if fs::symlink_metadata(&final_path).is_ok() {
                    self.verify_blob(digest, length)?;
                    return Ok(());
                }
                if self.used().checked_add(length).is_none_or(|n| n > maximum) {
                    return Err(StoreError::QuotaExceeded);
                }
                // Reserve BOTH temporary and published names: a crash can retain both hard links.
                if self
                    .retained_files()
                    .checked_add(2)
                    .is_none_or(|n| n > maximum_files as u64)
                {
                    return Err(StoreError::QuotaExceeded);
                }
                let temporary = match self {
                    Self::Volume { directory, .. } => directory
                        .join("temporary")
                        .join(uuid::Uuid::new_v4().to_string()),
                    _ => return Err(StoreError::Catalog),
                };
                let input = regular(&self.stage_path(id)?)?;
                let mut output = create(&temporary)?;
                if let Self::Volume { retained_files, .. } = self {
                    *retained_files += 1;
                }
                let copied = std::io::copy(&mut input.take(length), &mut output);
                let size = match output.metadata() {
                    Ok(metadata) => metadata.len(),
                    Err(_) => {
                        if let Self::Volume { uncertain, .. } = self {
                            *uncertain = true;
                        }
                        return Err(StoreError::Io);
                    }
                };
                if let Self::Volume { used, .. } = self {
                    *used += size;
                }
                if io(copied)? != length {
                    return Err(StoreError::Integrity);
                }
                io(output.sync_all())?;
                let linked = match fs::hard_link(&temporary, &final_path) {
                    Ok(()) => {
                        if let Self::Volume { retained_files, .. } = self {
                            *retained_files += 1;
                        }
                        true
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                        self.verify_blob(digest, length)?;
                        false
                    }
                    Err(_) => return Err(StoreError::Io),
                };
                sync_dir(final_path.parent().ok_or(StoreError::Catalog)?)?;
                io(fs::remove_file(&temporary))?;
                if let Self::Volume {
                    retained_files,
                    used,
                    ..
                } = self
                {
                    *retained_files -= 1;
                    if !linked {
                        *used -= size;
                    }
                }
                sync_dir(temporary.parent().ok_or(StoreError::Catalog)?)?;
                self.verify_blob(digest, length)?;
            }
        }
        Ok(())
    }
    pub fn verify_blob(&self, digest: Sha256Digest, length: u64) -> Result<(), StoreError> {
        let observed = match self {
            Self::Memory { blobs, .. } => {
                let bytes = blobs.get(&digest.as_hex()).ok_or(StoreError::Integrity)?;
                (
                    bytes.len() as u64,
                    Sha256Digest(Sha256::digest(bytes).into()),
                )
            }
            Self::Volume { .. } => {
                digest_reader(&mut regular(&self.blob_path(&digest.as_hex())?)?, length)?
            }
        };
        if observed != (length, digest) {
            return Err(StoreError::Integrity);
        }
        Ok(())
    }
    pub fn open_blob(&self, digest: Sha256Digest, length: u64) -> Result<ReadSource, StoreError> {
        self.verify_blob(digest, length)?;
        match self {
            Self::Memory { blobs, .. } => Ok(ReadSource::Memory(
                blobs
                    .get(&digest.as_hex())
                    .ok_or(StoreError::Integrity)?
                    .clone(),
            )),
            Self::Volume { .. } => Ok(ReadSource::File(regular(
                &self.blob_path(&digest.as_hex())?,
            )?)),
        }
    }
    pub fn abort_unacknowledged(&mut self, id: &str) -> Result<(), StoreError> {
        match self {
            Self::Memory { stages, used, .. } => {
                if let Some(data) = stages.remove(id) {
                    *used -= data.len() as u64;
                }
            }
            Self::Volume { .. } => {
                let path = self.stage_path(id)?;
                let size = io(regular(&path)?.metadata())?.len();
                io(fs::remove_file(&path))?;
                sync_dir(path.parent().ok_or(StoreError::Catalog)?)?;
                if let Self::Volume {
                    used,
                    retained_files,
                    ..
                } = self
                {
                    *used -= size;
                    *retained_files -= 1;
                }
            }
        }
        Ok(())
    }
}
