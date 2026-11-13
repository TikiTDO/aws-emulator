// SPDX-License-Identifier: CC0-1.0

//! Native Docker/OCI publication into the SAME store read by ECR and RegistryV2.
//! This is a buffered, bounded data plane: no host process, registry proxy or
//! filesystem escape. Native pushes do not yet publish EventBridge notifications;
//! the synchronous routing seam has no callback. JSON PutImage retains its events.

use super::registry_v2::{RegistryV2Target, registry_error as error, registry_headers};
use super::*;
use crate::state;

const MAX_MANIFEST_BYTES: usize = 4 * 1024 * 1024;
const UPLOAD_UNKNOWN: &str = "blob upload unknown";

type Store = ::engine::ServiceState<state::ComAmazonawsEcrAmazonec2containerregistryV20150921State>;

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    pub(super) fn serve_registry_push(
        &self,
        state: &Store,
        req: &RtRequest,
        target: &RegistryV2Target,
        principal: &str,
        now: std::time::SystemTime,
    ) -> RtResponse {
        let repository = match target {
            RegistryV2Target::Uploads { repository_name }
            | RegistryV2Target::Upload {
                repository_name, ..
            }
            | RegistryV2Target::Manifest {
                repository_name, ..
            } => repository_name,
            _ => return error(405, "UNSUPPORTED", "the operation is unsupported"),
        };
        if !valid_repository(repository) {
            return error(400, "NAME_INVALID", "invalid repository name");
        }
        let action = match (target, req.method()) {
            (RegistryV2Target::Manifest { .. }, _) => "PutImage",
            (RegistryV2Target::Uploads { .. }, _) => "InitiateLayerUpload",
            (_, "PUT") => "CompleteLayerUpload",
            _ => "UploadLayerPart",
        };
        if !state::authorization::registry_write_allowed(
            &*self.engine,
            principal,
            repository,
            action,
        ) {
            return error(403, "DENIED", "requested access to the resource is denied");
        }
        if state.lock().repository(repository).is_err() {
            return error(404, "NAME_UNKNOWN", "repository name not known to registry");
        }
        match target {
            RegistryV2Target::Uploads { .. } => {
                self.start_upload(state, req, repository, principal, now)
            }
            RegistryV2Target::Upload { upload_id, .. } => {
                self.change_upload(state, req, repository, upload_id, principal)
            }
            RegistryV2Target::Manifest { reference, .. } => {
                let _ = reference;
                error(501, "UNSUPPORTED", "use the streaming HTTP connector for manifest publication")
            }
            _ => error(405, "UNSUPPORTED", "the operation is unsupported"),
        }
    }

    fn start_upload(&self, _: &Store, _: &RtRequest, _: &str, _: &str, _: std::time::SystemTime) -> RtResponse {
        error(501, "UNSUPPORTED", "use the streaming HTTP connector for uploads")
    }
    fn change_upload(&self, _: &Store, _: &RtRequest, _: &str, _: &str, _: &str) -> RtResponse {
        error(501, "UNSUPPORTED", "use the streaming HTTP connector for uploads")
    }

    pub(super) async fn push_manifest(
        engine: &Eng,
        mutation: &state::payloads::MutationGuard,
        state: &Store,
        req: &RtRequest,
        repo: &str,
        reference: &str,
        now: std::time::SystemTime,
    ) -> RtResponse {
        let bytes = match body(req, MAX_MANIFEST_BYTES) {
            Ok(bytes) => bytes,
            Err(response) => return response,
        };
        let manifest = match std::str::from_utf8(bytes) {
            Ok(value) => value,
            Err(_) => return error(400, "MANIFEST_INVALID", "manifest must be UTF-8 JSON"),
        };
        let document: serde_json::Value = match serde_json::from_str(manifest) {
            Ok(document) => document,
            Err(_) => return error(400, "MANIFEST_INVALID", "manifest must be JSON"),
        };
        if document.get("schemaVersion").and_then(|v| v.as_u64()) != Some(2) {
            return error(
                400,
                "MANIFEST_INVALID",
                "only schema version 2 manifests and indexes are supported",
            );
        }
        let Some(media_type) = req
            .headers()
            .get("content-type")
            .map(|v| v.split(';').next().unwrap_or(v).trim())
        else {
            return error(400, "MANIFEST_INVALID", "manifest requires Content-Type");
        };
        let image = matches!(
            media_type,
            "application/vnd.docker.distribution.manifest.v2+json"
                | "application/vnd.oci.image.manifest.v1+json"
        );
        let index = matches!(
            media_type,
            "application/vnd.docker.distribution.manifest.list.v2+json"
                | "application/vnd.oci.image.index.v1+json"
        );
        if !image && !index {
            return error(415, "MANIFEST_INVALID", "unsupported manifest media type");
        }
        if document
            .get("mediaType")
            .and_then(|v| v.as_str())
            .is_some_and(|v| v != media_type)
        {
            return error(400, "MANIFEST_INVALID", "manifest media types disagree");
        }
        let digest = state::content_digest(bytes);
        let tag = if reference.contains(':') {
            if reference != digest {
                return error(
                    400,
                    "DIGEST_INVALID",
                    "manifest digest does not match content",
                );
            }
            None
        } else if valid_tag(reference) {
            Some(reference.to_string())
        } else {
            return error(400, "TAG_INVALID", "invalid manifest tag");
        };
        // The native boundary validates complete descriptor sets; no truncated
        // iterator or malformed document may be treated as a pullable image.
        let store = state.lock();
        let Ok(repository) = store.repository(repo) else {
            return error(404, "NAME_UNKNOWN", "repository name not known to registry");
        };
        let descriptors = if index {
            document
                .get("manifests")
                .and_then(|v| v.as_array())
                .filter(|v| v.len() <= state::MAX_MANIFEST_LIST_ENTRIES)
        } else {
            document
                .get("layers")
                .and_then(|v| v.as_array())
                .filter(|v| v.len() < state::MAX_MANIFEST_BLOB_ENTRIES)
        };
        let Some(descriptors) = descriptors else {
            return error(400, "MANIFEST_INVALID", "invalid descriptor array");
        };
        for descriptor in
            descriptors
                .iter()
                .chain(if image { document.get("config") } else { None })
        {
            let Some(child) = descriptor
                .get("digest")
                .and_then(|v| v.as_str())
                .filter(|v| state::is_layer_digest(v))
            else {
                return error(
                    400,
                    "MANIFEST_INVALID",
                    "descriptor requires a sha256 digest",
                );
            };
            let Some(size) = descriptor.get("size").and_then(|v| v.as_u64()) else {
                return error(400, "MANIFEST_INVALID", "descriptor requires a byte size");
            };
            let actual = if index {
                repository.images.get(child).map(|v| v.manifest.len())
            } else {
                repository.layer_blobs.get(child).and_then(|v| v.payload.as_ref()).map(|v| v.len() as usize)
            };
            if actual != usize::try_from(size).ok() {
                return error(
                    400,
                    "MANIFEST_BLOB_UNKNOWN",
                    "referenced content is missing or has a different size",
                );
            }
        }
        if image && document.get("config").is_none() {
            return error(
                400,
                "MANIFEST_INVALID",
                "image requires configuration descriptor",
            );
        }
        drop(store);
        let input = crate::ops::types::PutImageInput {
            image_manifest: manifest.to_string(),
            image_manifest_media_type: Some(media_type.to_string()),
            image_tag: tag,
            image_digest: Some(digest.clone()),
            repository_name: repo.to_string(),
            registry_id: None,
        };
        let signing = state::signing::planned_signings(engine, repo, now);
        let result = crate::ops::put_image::PutImage::<Eng>::store_pushed_image_guarded(
            engine, mutation, state,
            &input,
            digest.clone(),
            now,
            signing,
        ).await;
        use crate::ops::put_image::PutImageErrorModeled as E;
        use crate::scaffolded::Scaffolded;
        match result {
            Ok(_) | Err(Scaffolded::Modeled(E::ImageAlreadyExistsException(_))) => {
                // Deliberately no fabricated EventBridge acknowledgement here.
                let mut response = empty_response(201);
                response
                    .headers_mut()
                    .insert("location", format!("/v2/{repo}/manifests/{digest}"));
                response
                    .headers_mut()
                    .insert("docker-content-digest", digest);
                response
            }
            Err(Scaffolded::Modeled(E::ImageTagAlreadyExistsException(_))) => {
                error(409, "TAG_INVALID", "image tag is immutable")
            }
            Err(Scaffolded::Modeled(E::LayersNotFoundException(_))) => {
                error(400, "MANIFEST_BLOB_UNKNOWN", "referenced blob is missing")
            }
            Err(Scaffolded::Modeled(E::LimitExceededException(_))) => {
                error(413, "MANIFEST_INVALID", "registry capacity exceeded")
            }
            Err(_) => error(400, "MANIFEST_INVALID", "manifest could not be stored"),
        }
    }
}

fn body(req: &RtRequest, cap: usize) -> Result<&[u8], RtResponse> {
    let Some(bytes) = req.body().bytes() else {
        return Err(error(400, "BLOB_UPLOAD_INVALID", "buffered body required"));
    };
    if bytes.len() > cap {
        return Err(error(
            413,
            "BLOB_UPLOAD_INVALID",
            "content exceeds registry capacity",
        ));
    }
    Ok(bytes)
}

pub(super) fn range_matches(req: &RtRequest, start: usize, len: usize) -> bool {
    let Some(range) = req.headers().get("content-range") else {
        return true;
    };
    let range = range.strip_prefix("bytes ").unwrap_or(range);
    let Some((first, last)) = range.split_once('-') else {
        return false;
    };
    matches!((first.parse::<usize>(), last.parse::<usize>()), (Ok(first), Ok(last)) if first == start && last.checked_sub(first).and_then(|n| n.checked_add(1)) == Some(len))
}

pub(super) fn query(req: &RtRequest, name: &str) -> Result<Option<String>, RtResponse> {
    let Some((_, query)) = req.uri().split_once('?') else {
        return Ok(None);
    };
    let mut values = query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .filter(|(key, _)| *key == name);
    let value = values
        .next()
        .map(|(_, v)| ::smithy_server_runtime::percent_decode(v));
    if values.next().is_some() {
        return Err(error(
            400,
            "BLOB_UPLOAD_INVALID",
            "duplicate upload parameter",
        ));
    }
    Ok(value)
}

pub(super) fn empty_response(code: u16) -> RtResponse {
    let mut response = RtResponse::new(status(code), SdkBody::empty());
    registry_headers(&mut response);
    response.headers_mut().insert("content-length", "0");
    response
}

pub(super) fn upload_response(code: u16, repo: &str, id: &str, held: usize) -> RtResponse {
    let mut response = empty_response(code);
    response
        .headers_mut()
        .insert("location", format!("/v2/{repo}/blobs/uploads/{id}"));
    response
        .headers_mut()
        .insert("docker-upload-uuid", id.to_string());
    response
        .headers_mut()
        .insert("range", format!("0-{}", held.saturating_sub(1)));
    response
}

pub(super) fn blob_stored(repo: &str, digest: &str) -> RtResponse {
    let mut response = empty_response(201);
    response
        .headers_mut()
        .insert("location", format!("/v2/{repo}/blobs/{digest}"));
    response
        .headers_mut()
        .insert("docker-content-digest", digest.to_string());
    response
}

pub(super) fn valid_repository(name: &str) -> bool {
    name.len() <= 256
        && !name.is_empty()
        && name.split('/').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
        })
}

fn valid_tag(tag: &str) -> bool {
    tag.len() <= 128
        && !tag.is_empty()
        && tag.as_bytes()[0] != b'.'
        && tag.as_bytes()[0] != b'-'
        && tag
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}
