// SPDX-License-Identifier: CC0-1.0

use super::*;

/// The endpoint the request reached this emulator on, as every address this registry
/// ANNOUNCES names it — a layer download URL, a repository's own `repositoryUri` and the
/// registry endpoint a `docker login` is pointed at alike: the request's own `Host`, else
/// the authority its URI carries, else [`LOOPBACK_HOST`].
///
/// Read from the request rather than built from this registry's own
/// `<account>.dkr.ecr.<region>.amazonaws.com` host, because the address
/// has to resolve for whoever was handed it: that host is AWS's, and an address naming it
/// would send the next client out of the emulator entirely, while a client addressing
/// the emulator by socket is handed that socket.
///
/// The value is the CALLER's and it is interpolated into a URL this service announces, so
/// only a BARE AUTHORITY is honoured: anything else is not an endpoint, and the loopback
/// literal is announced instead of reflecting it.
///
/// The same rule, with the same two sources and the same filter, as
/// `service-com-amazonaws-artifact-artifact`'s `document_url_host` and
/// `service-com-amazonaws-iotdeviceadvisor-iotsenateservice`'s `report_url_host`: each
/// service resolves the caller's own endpoint for a URL it announces, and this note is
/// here so the three cannot drift apart unnoticed if any of them changes.
///
/// Each source is tested on its own, in that order, and the FIRST one carrying a bare
/// authority is what the URL names: a `Host` this registry cannot honour leaves the
/// authority the URI carries still to be read, rather than standing in for it.
pub fn caller_endpoint_host(parts: &::http::request::Parts) -> String {
    [
        parts
            .headers
            .get(::http::header::HOST)
            .and_then(|value| value.to_str().ok()),
        // The URI's AUTHORITY, not its host: a request that carries its endpoint on the
        // URI carries the PORT there too, and a URL announced without the port would not
        // resolve back to the socket the caller reached.
        parts.uri.authority().map(::http::uri::Authority::as_str),
    ]
    .into_iter()
    .flatten()
    .find(|candidate| is_bare_authority(candidate))
    .unwrap_or(LOOPBACK_HOST)
    .to_string()
}

/// `candidate` when it is an endpoint this emulator may announce itself at — a bare
/// authority — and the loopback literal when it is not.
///
/// The one filter behind every address this service announces: the value is a CALLER's and
/// it is interpolated into a URL or a header, so anything that is not an endpoint is
/// replaced by the literal rather than reflected.
pub fn honoured_endpoint_host(candidate: &str) -> &str {
    if is_bare_authority(candidate) {
        candidate
    } else {
        LOOPBACK_HOST
    }
}

/// True when `value` is a bare authority: a host with an optional port and nothing else.
///
/// A path, query, fragment, userinfo or space makes it something other than an endpoint —
/// a URL built around it would not resolve back to this emulator — so the parse is
/// `::http::uri::Authority`'s, with those separators refused first because an authority
/// accepts userinfo.
pub(super) fn is_bare_authority(value: &str) -> bool {
    !value.is_empty()
        && !value.contains(['/', '?', '#', '@', '\\', ' ', '\t'])
        && value.parse::<::http::uri::Authority>().is_ok()
}

/// The URL one layer download grant is served at, addressed to `host` (the endpoint the
/// caller reached this emulator on — see [`caller_endpoint_host`]) and scoped to
/// `region` (the Region the call asking for it was made against).
///
/// The path carries the grant's token, which is the capability a fetch resolves through,
/// and then the digest the grant serves, so the URL names the content it leads to the way
/// an object URL does. The query carries the SIGNATURE over that whole address — the six
/// query-string fields SigV4 signs a download with — so the window the URL states is a
/// window a fetch is judged against rather than a statement about one: rewriting any of
/// those fields presents a signature that was not computed over what was presented, which
/// is what [`layer_download_signature`] refuses.
pub fn layer_download_url(
    host: &str,
    region: &str,
    token: &str,
    grant: &LayerDownloadGrant,
) -> String {
    let target = format!(
        "{LAYER_DOWNLOAD_PATH_PREFIX}/{token}/{}",
        grant.layer_digest
    );
    presigned_layer_download_url(
        host,
        &target,
        region,
        grant.issued_at,
        Duration::from_secs(grant.validity_seconds()),
    )
}

// ---------------------------------------------------------------------------
// The signature a published layer download URL carries.
//
// `GetDownloadUrlForLayer`'s `downloadUrl` is a PRE-SIGNED download address: what makes
// it followable by a client holding no credential of its own is the query-string
// signature over that address, and what bounds it is that the window it states is one of
// the SIGNED fields. So the signature is computed by the maintained `aws-sigv4` crate
// over the very address announced, and `crate::service`'s download plane verifies it
// before it resolves the grant — a rewritten field, another address, another endpoint
// host or a window extended by hand is a signature nothing computed, and is refused.
//
// The framework's own request verifier is deliberately NOT the verifier here: it checks a
// CALLER's SigV4 against the credentials the emulator minted and holds in its credential
// store, while the URL below is signed with this download plane's own ephemeral key,
// which that store neither holds nor should. What the two share is the computation, and
// that is the `aws-sigv4` crate both use rather than a signing routine written here.
//
// The credential scope names THIS service rather than the object store a real download is
// served from: the URL is served by this emulator, so the scope has to name the service a
// fetch of it is routed to, and a scope naming another service's signing name would send
// the front door looking for that service.
// ---------------------------------------------------------------------------

/// The signing name a layer download URL's credential scope carries: this service's own,
/// because this emulator is what serves the download.
pub(super) const LAYER_DOWNLOAD_SIGNING_NAME: &str = "ecr";

/// The most query fields a layer download request target may carry.
///
/// A URL this registry mints carries exactly the six SigV4 signing fields and none of a
/// client's own, so this is those plus a little room: a target carrying more is one this
/// registry did not announce, and refusing it on the count alone keeps what the
/// verification costs independent of how many fields a caller sends.
pub(super) const MAX_LAYER_DOWNLOAD_QUERY_FIELDS: usize = 8;

/// The longest credential-scope Region this plane reads back off a presented URL. A
/// Region name is a short label, so a longer one is not one any scope names — bounded
/// here because the value is a caller's and it is fed to the signer.
pub(super) const MAX_SIGNED_REGION_LENGTH: usize = 64;

/// `host` + `target` with the query-string signature that makes it a download address
/// anyone holding it can follow: `X-Amz-Algorithm`, `X-Amz-Credential`, `X-Amz-Date`,
/// `X-Amz-Expires`, `X-Amz-SignedHeaders` and `X-Amz-Signature`, as `aws-sigv4` computes
/// them for a `GET` of that address over the host it names.
///
/// `signed_at` and `lifetime` are parameters rather than read from the clock so that the
/// instant a URL was signed at is the caller's: [`layer_download_url`] signs at the
/// instant the grant was minted, and [`layer_download_signature`] re-signs at the instant
/// the presented URL states, which is what lets one computation both mint and verify.
///
/// An address the signer cannot be built for is announced UNSIGNED, and the download plane
/// then refuses it rather than serving a layer from an address whose signature nothing
/// computed.
pub fn presigned_layer_download_url(
    host: &str,
    target: &str,
    region: &str,
    signed_at: SystemTime,
    lifetime: Duration,
) -> String {
    presign_layer_download(host, target, region, signed_at, lifetime).unwrap_or_else(|| {
        ::tracing::warn!(%target, "a layer download address could not be signed");
        format!("https://{host}{target}")
    })
}

/// What the signature a layer download request presents IS: the verdict
/// `crate::service`'s download plane serves or refuses on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerDownloadSignature {
    /// Signed by this registry, over this address, and inside the window it states.
    Honoured,
    /// No query-string signature at all.
    Absent,
    /// Signed by this registry, but the window the URL states has closed.
    Expired,
    /// A signature that does not verify over the address it was presented on: a field was
    /// rewritten, the address was, or it was signed by something else.
    Invalid,
}

/// Whether the address a layer download request presents is one this registry signed and
/// still honours.
///
/// `target` is the request target as it arrived (path and query) and `host` the endpoint
/// it arrived on — the two halves of the address the signature covers. The address is
/// honoured when it is FIELD FOR FIELD the one this registry would mint over that target,
/// at the instant and for the window the URL itself states: the signing fields are
/// stripped, the rest of the query travels into the address being re-signed, and what
/// comes back is compared with what was presented. So a rewritten algorithm, scope,
/// timestamp, window, signed-header list or signature, a field of the client's own,
/// another endpoint host and another layer's address are each an address this registry did
/// not mint. A window longer than [`LAYER_DOWNLOAD_URL_TTL`] is not one it ever mints
/// either.
///
/// The address is signed for the download of a layer, so a `HEAD` of it — which reports
/// what a `GET` of the same address would answer ABOUT that layer — is checked against
/// the same signature.
pub fn layer_download_signature(
    host: &str,
    target: &str,
    now: SystemTime,
) -> LayerDownloadSignature {
    let Some((path, query)) = target.split_once('?') else {
        return LayerDownloadSignature::Absent;
    };
    // The field count is the one part of this check a CALLER chooses, so it is capped
    // before the fields are even collected.
    if query.split('&').count() > MAX_LAYER_DOWNLOAD_QUERY_FIELDS {
        return LayerDownloadSignature::Invalid;
    }
    let presented = query_fields(query);
    if !presented
        .iter()
        .any(|(name, _value)| *name == "X-Amz-Signature")
    {
        return LayerDownloadSignature::Absent;
    }
    let stated = |name: &str| {
        presented
            .iter()
            .find_map(|(field, value)| (*field == name).then_some(*value))
    };
    let (Some(signed_at), Some(expires), Some(credential)) = (
        stated("X-Amz-Date"),
        stated("X-Amz-Expires"),
        stated("X-Amz-Credential"),
    ) else {
        return LayerDownloadSignature::Invalid;
    };
    let (Some(signed_at), Ok(seconds), Some(region)) = (
        signing_instant(signed_at),
        expires.parse::<u64>(),
        // The Region the presented scope names, so the key the signature is re-derived
        // with is the one it claims to have been minted under: a scope naming another
        // Region re-signs to another signature, which is exactly a mismatch.
        scope_region(credential),
    ) else {
        return LayerDownloadSignature::Invalid;
    };
    let lifetime = Duration::from_secs(seconds);
    if lifetime > LAYER_DOWNLOAD_URL_TTL {
        return LayerDownloadSignature::Invalid;
    }
    let Some(expires_at) = signed_at.checked_add(lifetime) else {
        return LayerDownloadSignature::Invalid;
    };
    // Every field the signing adds back is dropped; everything else is part of the address
    // the signature covers, so a field a client appended of its own travels into it.
    let carried: Vec<String> = presented
        .iter()
        .filter(|(name, _value)| !name.starts_with("X-Amz-"))
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    let signed_target = if carried.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{}", carried.join("&"))
    };
    let minted = presign_layer_download(host, &signed_target, &region, signed_at, lifetime);
    let Some((_address, minted_query)) = minted
        .as_deref()
        .and_then(|minted| minted.split_once('?'))
    else {
        return LayerDownloadSignature::Invalid;
    };
    if !same_query(&query_fields(minted_query), &presented) {
        return LayerDownloadSignature::Invalid;
    }
    // The window is answered only for an address that IS this registry's: one it never
    // minted is not expired, it is invalid.
    if now >= expires_at {
        LayerDownloadSignature::Expired
    } else {
        LayerDownloadSignature::Honoured
    }
}

/// The query fields of `query`, in the order they arrived, as (name, value) pairs. The
/// values stay exactly as they arrived, because a signature is over the address as it was
/// WRITTEN: a field re-encoded by a client is a different canonical request.
pub(super) fn query_fields(query: &str) -> Vec<(&str, &str)> {
    query
        .split('&')
        .filter(|field| !field.is_empty())
        .map(|field| field.split_once('=').unwrap_or((field, "")))
        .collect()
}

/// Whether `presented` is the address `minted` field for field: the same fields, each with
/// the same value, in any order — which is the order-independence SigV4's canonical query
/// string itself has.
pub(super) fn same_query(minted: &[(&str, &str)], presented: &[(&str, &str)]) -> bool {
    if minted.len() != presented.len() {
        return false;
    }
    let presented: BTreeMap<&str, &str> = presented.iter().copied().collect();
    minted.iter().all(|(name, value)| {
        presented.get(name).is_some_and(|presented| {
            // The signature is the field derived from this plane's own key, so how long its
            // comparison takes must not depend on how much of a presented one was right.
            if *name == "X-Amz-Signature" {
                same_signature(value, presented)
            } else {
                presented == value
            }
        })
    })
}

/// `expected == presented`, without stopping at the first byte that differs.
pub(super) fn same_signature(expected: &str, presented: &str) -> bool {
    let (expected, presented) = (expected.as_bytes(), presented.as_bytes());
    expected.len() == presented.len()
        && expected
            .iter()
            .zip(presented)
            .fold(0u8, |difference, (left, right)| difference | (left ^ right))
            == 0
}

/// The Region a presented credential scope (`<key>/<date>/<region>/<name>/aws4_request`)
/// names, or `None` when the field is not a scope of that shape at all.
///
/// The `/` separators are percent-encoded in a query field, so the value is decoded first
/// — through the same decoding [`parse_layer_download_path`] reads a path segment with.
pub(super) fn scope_region(credential: &str) -> Option<String> {
    let decoded = ::smithy_server_runtime::percent_decode(credential);
    let scope: Vec<&str> = decoded.split('/').collect();
    match scope.as_slice() {
        [_key, _date, region, _name, terminator]
            if *terminator == "aws4_request"
                && !region.is_empty()
                && region.len() <= MAX_SIGNED_REGION_LENGTH =>
        {
            Some((*region).to_string())
        }
        _ => None,
    }
}

/// The signing of one layer download address, shared by the mint and the verification so
/// there is one computation and no second copy of what a signature covers.
///
/// `None` when the parameters the crate needs cannot be built from the address.
pub(super) fn presign_layer_download(
    host: &str,
    target: &str,
    region: &str,
    signed_at: SystemTime,
    lifetime: Duration,
) -> Option<String> {
    use ::aws_sigv4::http_request::{
        sign, PayloadChecksumKind, SignableBody, SignableRequest, SignatureLocation,
        SigningSettings,
    };

    let address = format!("https://{host}{target}");
    let identity: ::aws_smithy_runtime_api::client::identity::Identity =
        ::aws_credential_types::Credentials::builder()
            .provider_name(LAYER_DOWNLOAD_SIGNING_NAME)
            .access_key_id(download_signing_key_id())
            .secret_access_key(download_signing_key())
            .build()
            .into();
    let mut settings = SigningSettings::default();
    // The signature travels in the query string, which is what makes the address
    // followable by a client that sends no `Authorization` header at all.
    settings.signature_location = SignatureLocation::QueryParams;
    settings.expires_in = Some(lifetime);
    settings.payload_checksum_kind = PayloadChecksumKind::NoHeader;
    let params = ::aws_sigv4::sign::v4::SigningParams::builder()
        .identity(&identity)
        .region(region)
        .name(LAYER_DOWNLOAD_SIGNING_NAME)
        .settings(settings)
        .time(signed_at)
        .build()
        .ok()?;
    let signable = SignableRequest::new(
        "GET",
        &address,
        ::std::iter::once(("host", host)),
        SignableBody::UnsignedPayload,
    )
    .ok()?;
    let (instructions, _signature) = sign(signable, &params.into()).ok()?.into_parts();
    let mut request = ::http::Request::builder()
        .method("GET")
        .uri(&address)
        .body(())
        .ok()?;
    instructions.apply_to_request_http1x(&mut request);
    Some(request.uri().to_string())
}

/// The key this registry's layer download plane signs a published URL with.
///
/// Drawn once per process from [`layer_download_signing_seed`] and digested with `sha2`:
/// it is the plane's own signing key, not a credential of any account and not the
/// emulator's own — nothing returns it, nothing else accepts it, and it is gone when the
/// process is, so a URL one run announced is not followable against another.
pub(super) fn download_signing_key() -> &'static str {
    static KEY: ::std::sync::OnceLock<String> = ::std::sync::OnceLock::new();
    KEY.get_or_init(|| signing_digest_of("layer-download-signing-key"))
        .as_str()
}

/// The id of that key, as a published URL's `X-Amz-Credential` names the signer: the
/// 20-character shape of a temporary access key id, derived from the process seed under
/// its own domain so it identifies the signing key without revealing it.
pub(super) fn download_signing_key_id() -> &'static str {
    static KEY_ID: ::std::sync::OnceLock<String> = ::std::sync::OnceLock::new();
    KEY_ID
        .get_or_init(|| {
            format!(
                "ASIA{}",
                signing_digest_of("layer-download-key-id")[..16].to_ascii_uppercase()
            )
        })
        .as_str()
}

/// This process's signing seed digested under `domain`, hex-rendered: one derivation, so
/// two values of this module are never the same bytes under two names.
pub(super) fn signing_digest_of(domain: &str) -> String {
    use ::sha2::Digest as _;

    let mut digest = ::sha2::Sha256::new();
    digest.update(layer_download_signing_seed().to_be_bytes());
    digest.update(domain.as_bytes());
    ::hex::encode(digest.finalize())
}

/// A per-process random value, the part of the signing key that makes it UNGUESSABLE.
///
/// `RandomState` seeds each of its hashers with keys the standard library draws from the
/// OS once per process, so hashing a fixed value through one yields a value that differs
/// between runs and is not derivable from any request — the same source [`mint_hex`] takes
/// an unguessable token from.
pub(super) fn layer_download_signing_seed() -> u64 {
    static SEED: ::std::sync::OnceLock<u64> = ::std::sync::OnceLock::new();
    *SEED.get_or_init(|| {
        let mut hasher = ::std::hash::BuildHasher::build_hasher(
            &::std::collections::hash_map::RandomState::new(),
        );
        ::std::hash::Hash::hash(&"com.amazonaws.ecr#layer-download", &mut hasher);
        ::std::hash::Hasher::finish(&hasher)
    })
}

/// The instant a published URL's `X-Amz-Date` names: `YYYYMMDDTHHMMSSZ`, the basic form
/// SigV4 states it in.
///
/// The fixed-width fields are re-rendered as the extended form and read by
/// `aws_smithy_types`' own date-time parser — the type every modelled member of this
/// service is serialized through — rather than by calendar arithmetic of this module's
/// own.
pub(super) fn signing_instant(stamp: &str) -> Option<SystemTime> {
    let bytes = stamp.as_bytes();
    if bytes.len() != 16 || bytes[8] != b'T' || bytes[15] != b'Z' {
        return None;
    }
    if !bytes[..8]
        .iter()
        .chain(&bytes[9..15])
        .all(u8::is_ascii_digit)
    {
        return None;
    }
    let extended = format!(
        "{}-{}-{}T{}:{}:{}Z",
        stamp.get(0..4)?,
        stamp.get(4..6)?,
        stamp.get(6..8)?,
        stamp.get(9..11)?,
        stamp.get(11..13)?,
        stamp.get(13..15)?
    );
    let at = ::aws_smithy_types::DateTime::from_str(
        &extended,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .ok()?;
    SystemTime::try_from(at).ok()
}

/// What a request to a published layer download URL addresses: the grant token the URL
/// carries, and the digest its path names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerDownloadTarget {
    /// The token the URL carries, which addresses the grant in
    /// [`ComAmazonawsEcrAmazonec2containerregistryV20150921State::layer_download_grants`].
    pub token: String,
    /// The layer digest the URL's path names, which has to be the one the grant serves.
    pub layer_digest: String,
}

/// The layer download a request path addresses, or `None` when the path is not one
/// [`layer_download_url`] publishes.
///
/// Both segments are percent-decoded, so a client that encoded the digest's `:` resolves
/// the same URL as one that sent it literally (a `:` is legal in a path segment, so this
/// service announces it unencoded).
pub fn parse_layer_download_path(path: &str) -> Option<LayerDownloadTarget> {
    let rest = path
        .strip_prefix(LAYER_DOWNLOAD_PATH_PREFIX)?
        .strip_prefix('/')?;
    let (token, digest) = rest.split_once('/')?;
    (!token.is_empty() && !digest.is_empty() && !digest.contains('/')).then(|| {
        LayerDownloadTarget {
            token: ::smithy_server_runtime::percent_decode(token),
            layer_digest: ::smithy_server_runtime::percent_decode(digest),
        }
    })
}

/// A freshly minted hex id of `bytes` unguessable bytes, at most the 32 a SHA-256 is
/// wide; a wider request is served the whole digest.
///
/// The bytes are the leading ones of a SHA-256 — the crate's existing `sha2` dependency,
/// the same one that digests a manifest — over three inputs: a per-process random seed
/// from the standard library's `RandomState` (the OS-seeded randomness `HashMap` uses), a
/// monotonically increasing counter, and the clock. The seed keeps two processes apart,
/// and the counter keeps two ids of one process apart even if the clock does not advance,
/// so no two ids minted here collide. Nothing in an id is derived from the request.
///
/// The one mint in this service: an `InitiateLayerUpload` upload id and a layer download
/// token are both ids a later call addresses a record by, so neither has its own recipe.
pub fn mint_hex(bytes: usize) -> String {
    use ::sha2::Digest as _;
    use ::std::hash::{BuildHasher as _, Hasher as _};

    static MINTED: ::std::sync::atomic::AtomicU64 = ::std::sync::atomic::AtomicU64::new(0);

    let counter = MINTED.fetch_add(1, ::std::sync::atomic::Ordering::Relaxed);
    let mut seeder = ::std::collections::hash_map::RandomState::new().build_hasher();
    seeder.write_u64(counter);
    let nanos = SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_nanos());

    let mut digest = ::sha2::Sha256::new();
    digest.update(seeder.finish().to_be_bytes());
    digest.update(counter.to_be_bytes());
    digest.update(nanos.to_be_bytes());
    let minted = digest.finalize();
    ::hex::encode(&minted[..bytes.min(minted.len())])
}
