// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Process-stable capacity shared by the production bridge and ECR byte writers.
//! Limits are emulator policy, not AWS quotas. Configuration errors never fall back.

use std::sync::OnceLock;

pub const LAYER_ENV: &str = "EMULATOR_ECR_MAX_LAYER_BYTES";
pub const STORE_ENV: &str = "EMULATOR_ECR_MAX_STORE_BYTES";
pub const DEFAULT_LAYER_BYTES: usize = 128 * 1024 * 1024;
pub const DEFAULT_STORE_BYTES: usize = 512 * 1024 * 1024;
const MAX_LAYER_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_STORE_BYTES: u64 = 8 * 1024 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EcrCapacity {
    pub layer_bytes: usize,
    pub store_bytes: usize,
}

impl EcrCapacity {
    /// Strict decimal bytes; pure so tests do not mutate process environment.
    pub fn parse(layer: Option<&str>, store: Option<&str>) -> Result<Self, String> {
        fn value(raw: Option<&str>, default: usize, max: u64, name: &str) -> Result<usize, String> {
            let Some(raw) = raw else { return Ok(default) };
            let parsed = if !raw.is_empty() && raw.bytes().all(|b| b.is_ascii_digit()) {
                raw.parse::<u64>().ok()
            } else { None };
            parsed.filter(|v| *v > 0 && *v <= max)
                .and_then(|v| usize::try_from(v).ok())
                .ok_or_else(|| format!("{name} must be decimal bytes in 1..={max} representable on this host"))
        }
        let layer_bytes = value(layer, DEFAULT_LAYER_BYTES, MAX_LAYER_BYTES, LAYER_ENV)?;
        let store_bytes = value(store, DEFAULT_STORE_BYTES, MAX_STORE_BYTES, STORE_ENV)?;
        if layer_bytes > store_bytes {
            return Err(format!("{STORE_ENV} must be at least {LAYER_ENV}"));
        }
        Ok(Self { layer_bytes, store_bytes })
    }

    pub fn layer_fits(self, held: usize, additional: usize) -> bool {
        held.checked_add(additional).is_some_and(|n| n <= self.layer_bytes)
    }

    pub fn store_fits(self, held: usize, additional: usize) -> bool {
        held.checked_add(additional).is_some_and(|n| n <= self.store_bytes)
    }
}

/// Read only the two capacity settings once, including caching explicit failure.
pub fn configured() -> Result<&'static EcrCapacity, &'static str> {
    static CONFIG: OnceLock<Result<EcrCapacity, String>> = OnceLock::new();
    CONFIG.get_or_init(|| {
        fn read(name: &str) -> Result<Option<String>, String> {
            match std::env::var(name) {
                Ok(value) => Ok(Some(value)),
                Err(std::env::VarError::NotPresent) => Ok(None),
                Err(std::env::VarError::NotUnicode(_)) => Err(format!("{name} must be Unicode decimal bytes")),
            }
        }
        let layer = read(LAYER_ENV)?;
        let store = read(STORE_ENV)?;
        EcrCapacity::parse(layer.as_deref(), store.as_deref())
    }).as_ref().map_err(String::as_str)
}

/// Pre-auth resource admission only. Actual routing/authentication still runs later.
/// Only canonical native upload paths receive a larger body allowance.
pub(crate) fn native_upload(parts: &http::request::Parts) -> bool {
    let Some(rest) = parts.uri.path().strip_prefix("/v2/") else { return false };
    let Some((repo, suffix)) = rest.rsplit_once("/blobs/uploads") else { return false };
    if repo.is_empty() || repo.len() > 256 || !repo.split('/').all(|s| {
        !s.is_empty() && s != "." && s != ".." && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
    }) { return false; }
    match parts.method {
        http::Method::POST => suffix.is_empty() || suffix == "/",
        http::Method::PATCH | http::Method::PUT => suffix.strip_prefix('/').is_some_and(|id| {
            !id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        }),
        _ => false,
    }
}

/// Serialize registry byte writers before collecting their wire bodies. JSON stays
/// at the ordinary wire limit; it shares the ECR decoded-byte store policy.
pub(crate) fn byte_writer(parts: &http::request::Parts) -> bool {
    if native_upload(parts) { return true; }
    if parts.method == http::Method::PUT && parts.uri.path().starts_with("/v2/")
        && parts.uri.path().contains("/manifests/") { return true; }
    parts.method == http::Method::POST && parts.headers.get("x-amz-target")
        .and_then(|v| v.to_str().ok()).is_some_and(|target| matches!(target,
            "AmazonEC2ContainerRegistry_V20150921.UploadLayerPart" |
            "AmazonEC2ContainerRegistry_V20150921.PutImage"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capacity_parser_and_shared_accounting() {
        assert_eq!(EcrCapacity::parse(None, None).unwrap(), EcrCapacity { layer_bytes: DEFAULT_LAYER_BYTES, store_bytes: DEFAULT_STORE_BYTES });
        let cap = EcrCapacity::parse(Some("4"), Some("8")).unwrap();
        assert!(cap.layer_fits(3, 1));
        assert!(!cap.layer_fits(3, 2));
        assert!(cap.store_fits(4, 4));
        assert!(!cap.store_fits(4, 5));
        assert!(!cap.store_fits(usize::MAX, 1));
        for bad in ["", "0", "-1", "+1", " 4", "4MiB", "18446744073709551616", "1073741825"] {
            assert!(EcrCapacity::parse(Some(bad), None).is_err());
        }
        assert!(EcrCapacity::parse(Some("9"), Some("8")).is_err());
        assert!(EcrCapacity::parse(None, Some("8589934593")).is_err());
    }
    #[test]
    fn only_native_uploads_gain_body_capacity() {
        for (method, path, expected) in [
            ("POST", "/v2/team/repo/blobs/uploads/?digest=sha256:abc", true),
            ("PATCH", "/v2/team/repo/blobs/uploads/abc-123", true),
            ("PUT", "/v2/repo/blobs/uploads/abc?digest=sha256:abc", true),
            ("GET", "/v2/repo/blobs/uploads/abc", false),
            ("PUT", "/v2/repo/manifests/latest", false),
            ("PATCH", "/v2/repo/blobs/uploads/abc/extra", false),
            ("POST", "/v2/../blobs/uploads/", false),
            ("POST", "/other", false),
        ] {
            let (parts, _) = http::Request::builder().method(method).uri(path).body(()).unwrap().into_parts();
            assert_eq!(native_upload(&parts), expected, "{method} {path}");
        }
        let (parts, _) = http::Request::builder().method("POST").uri("/")
            .header("x-amz-target", "AmazonEC2ContainerRegistry_V20150921.UploadLayerPart")
            .body(()).unwrap().into_parts();
        assert!(byte_writer(&parts));
        assert!(!native_upload(&parts));
    }
}
