// SPDX-License-Identifier: CC0-1.0

//! Incremental entity authentication and AWS chunk decoding. Output is private staging data,
//! never a publication grant. The driver must consume the actual HTTP EOF before `finish`.

use super::*;
use aws_lc_rs::digest;
use aws_smithy_checksums::{ChecksumAlgorithm, http::HttpChecksum};
use std::sync::Arc;

const MAX_LINE_BYTES: usize = 8192;
const MAX_TRAILER_BYTES: usize = 16384;
const MAX_TRAILERS: usize = 8;
const TRAILER_SIGNATURE: &str = "x-amz-trailer-signature";
const CHECKSUMS: [(&str, &str); 5] = [
    ("x-amz-checksum-crc32", "crc32"),
    ("x-amz-checksum-crc32c", "crc32c"),
    ("x-amz-checksum-crc64nvme", "crc64nvme"),
    ("x-amz-checksum-sha1", "sha1"),
    ("x-amz-checksum-sha256", "sha256"),
];

#[derive(Debug, PartialEq, Eq)]
pub enum StreamAuthenticationError {
    Unsupported,
    Denied(Denied),
}
#[derive(Debug, PartialEq, Eq)]
pub enum StreamIntegrityFailure {
    MalformedChecksum,
    ChecksumMismatch,
    DecodedLengthMismatch,
    TrailerDeclarationMismatch,
}
#[derive(Debug, PartialEq, Eq)]
pub enum StreamValidationError {
    Authentication(Denied),
    MalformedFraming(&'static str),
    Integrity(StreamIntegrityFailure),
    LimitExceeded,
}
impl std::fmt::Display for StreamAuthenticationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported => {
                f.write_str("incremental authentication does not support this body mode")
            }
            Self::Denied(denied) => write!(f, "{}: {}", denied.code, denied.message),
        }
    }
}
impl std::error::Error for StreamAuthenticationError {}
impl std::fmt::Display for StreamValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Authentication(denied) => write!(f, "{}: {}", denied.code, denied.message),
            Self::MalformedFraming(message) => f.write_str(message),
            Self::Integrity(kind) => write!(f, "payload integrity failure: {kind:?}"),
            Self::LimitExceeded => f.write_str("payload framing limit exceeded"),
        }
    }
}
impl std::error::Error for StreamValidationError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadSignatureBinding {
    Sha256,
    Unsigned,
    SignedChunks,
}

pub struct PayloadStep<'a> {
    pub consumed: usize,
    pub payload: Option<&'a [u8]>,
}

/// No credentials or caller-controlled metadata are carried in this terminal evidence.
/// Service authorization, Content-MD5 and publication remain the owning service's checks.
#[derive(Debug)]
pub struct StreamAuthentication {
    entity_bytes: u64,
    decoded_bytes: u64,
    decoded_sha256: [u8; 32],
    binding: PayloadSignatureBinding,
    checksums: Vec<(&'static str, String)>,
}
impl StreamAuthentication {
    pub fn entity_bytes(&self) -> u64 {
        self.entity_bytes
    }
    pub fn decoded_bytes(&self) -> u64 {
        self.decoded_bytes
    }
    pub fn decoded_sha256(&self) -> &[u8; 32] {
        &self.decoded_sha256
    }
    pub fn binding(&self) -> &PayloadSignatureBinding {
        &self.binding
    }
    /// Validated decoded-payload checksum values, including embedded trailers, for the
    /// service's metadata and checksum-cardinality policy. No credentials are included.
    pub fn checksums(&self) -> &[(&'static str, String)] {
        &self.checksums
    }
}

pub trait RequestAuthenticationStream: Send {
    /// Each nonempty successful step consumes at least one byte. Await staging the returned
    /// decoded span before advancing the input. Any error poisons the stream permanently.
    fn consume<'a>(
        &mut self,
        entity_bytes: &'a [u8],
    ) -> Result<PayloadStep<'a>, StreamValidationError>;
    /// Call only after real EOF, with the HTTP library's actual trailers (if any).
    fn finish(
        self: Box<Self>,
        http_trailers: Option<&http::HeaderMap>,
    ) -> Result<StreamAuthentication, StreamValidationError>;
}

#[derive(Clone, Copy)]
enum Mode {
    Hashed,
    Unsigned,
    StreamingUnsigned,
    StreamingSigned { trailers: bool },
}
impl Mode {
    fn marker(self) -> Option<&'static str> {
        match self {
            Self::Hashed => None,
            Self::Unsigned => Some("UNSIGNED-PAYLOAD"),
            Self::StreamingUnsigned => Some("STREAMING-UNSIGNED-PAYLOAD-TRAILER"),
            Self::StreamingSigned { trailers: false } => Some("STREAMING-AWS4-HMAC-SHA256-PAYLOAD"),
            Self::StreamingSigned { trailers: true } => {
                Some("STREAMING-AWS4-HMAC-SHA256-PAYLOAD-TRAILER")
            }
        }
    }
    fn signed_chunks(self) -> bool {
        matches!(self, Self::StreamingSigned { .. })
    }
    fn signed_trailers(self) -> bool {
        matches!(self, Self::StreamingSigned { trailers: true })
    }
    fn requires_trailers(self) -> bool {
        matches!(
            self,
            Self::StreamingUnsigned | Self::StreamingSigned { trailers: true }
        )
    }
}

struct Credentials {
    authenticator: &'static SigV4Authenticator,
    store: Arc<CredentialStore>,
    serving: String,
}
// Deliberately not Debug/Serialize. Signing material lives only for this request.
struct Chain {
    signed: Signature,
    secret: String,
    previous: String,
}
struct ChecksumCheck {
    name: &'static str,
    expected: Option<Vec<u8>>,
    trailer: bool,
    calculator: Box<dyn HttpChecksum>,
}
#[derive(Clone, Copy)]
enum State {
    Plain,
    Header,
    Data(u64),
    DataCrlf(u8),
    Trailers,
    Complete,
}
struct Stream {
    parts: Parts,
    credentials: Option<Credentials>,
    chain: Option<Chain>,
    mode: Mode,
    state: State,
    failed: bool,
    line: Vec<u8>,
    chunk_signature: Option<String>,
    chunk_hash: digest::Context,
    entity_hash: digest::Context,
    decoded_hash: digest::Context,
    entity_bytes: u64,
    decoded_bytes: u64,
    expected_entity: Option<u64>,
    expected_decoded: Option<u64>,
    checksums: Vec<ChecksumCheck>,
    trailers: Vec<(String, String)>,
    trailer_bytes: usize,
}

fn framing(message: &'static str) -> StreamValidationError {
    StreamValidationError::MalformedFraming(message)
}
fn integrity(kind: StreamIntegrityFailure) -> StreamValidationError {
    StreamValidationError::Integrity(kind)
}
fn hash() -> digest::Context {
    digest::Context::new(&digest::SHA256)
}
fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 15) as usize] as char);
    }
    out
}
fn duplicate(parts: &Parts, name: &str) -> bool {
    parts.headers.get_all(name).iter().count() > 1
}
fn header<'a>(parts: &'a Parts, name: &str) -> Result<Option<&'a str>, StreamAuthenticationError> {
    if duplicate(parts, name) {
        return Err(StreamAuthenticationError::Unsupported);
    }
    parts
        .headers
        .get(name)
        .map(|v| {
            v.to_str()
                .map_err(|_| StreamAuthenticationError::Unsupported)
        })
        .transpose()
}
fn length(parts: &Parts, name: &str) -> Result<Option<u64>, StreamAuthenticationError> {
    header(parts, name)?
        .map(|v| {
            if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) {
                return Err(StreamAuthenticationError::Unsupported);
            }
            v.parse()
                .map_err(|_| StreamAuthenticationError::Unsupported)
        })
        .transpose()
}
fn owned_parts(parts: &Parts) -> Parts {
    let (mut owned, _) = http::Request::new(()).into_parts();
    owned.method = parts.method.clone();
    owned.uri = parts.uri.clone();
    owned.version = parts.version;
    owned.headers = parts.headers.clone();
    owned
}

pub(super) fn begin(
    authenticator: &'static SigV4Authenticator,
    parts: &Parts,
    serving: &str,
    store: Arc<CredentialStore>,
) -> Result<Box<dyn RequestAuthenticationStream>, StreamAuthenticationError> {
    let (signed, secret) = checked_signature_credentials(authenticator, parts, serving, &store)
        .map_err(StreamAuthenticationError::Denied)?;
    let presigned_s3 = signed.expires_secs.is_some() && signs_like_s3(signed.signing_name());
    let mut stream = Stream::new(parts, presigned_s3)?;
    if let Some(marker) = stream.mode.marker() {
        let expected = signature_for_payload(
            parts,
            &signed,
            &secret,
            SignableBody::Precomputed(marker.into()),
        );
        if !expected.is_some_and(|expected| same_secret(&expected, &signed.value)) {
            return Err(StreamAuthenticationError::Denied(SIGNATURE_MISMATCH));
        }
    }
    if stream.mode.signed_chunks() {
        stream.chain = Some(Chain {
            previous: signed.value.clone(),
            signed,
            secret,
        });
    }
    stream.credentials = Some(Credentials {
        authenticator,
        store,
        serving: serving.into(),
    });
    Ok(Box::new(stream))
}

/// Only an explicit anonymous/auth-disabled/native admission owner may choose this path.
/// It validates framing/checksums, not request credentials. Signed chunks need a verifier.
pub fn begin_uncredentialed_stream(
    parts: &Parts,
) -> Result<Box<dyn RequestAuthenticationStream>, StreamAuthenticationError> {
    let stream = Stream::new(parts, false)?;
    if stream.mode.signed_chunks() {
        return Err(StreamAuthenticationError::Unsupported);
    }
    Ok(Box::new(stream))
}

impl Stream {
    fn new(parts: &Parts, presigned_s3: bool) -> Result<Self, StreamAuthenticationError> {
        let mode = match header(parts, X_AMZ_CONTENT_SHA256)? {
            Some("UNSIGNED-PAYLOAD") => Mode::Unsigned,
            Some("STREAMING-UNSIGNED-PAYLOAD-TRAILER") => Mode::StreamingUnsigned,
            Some("STREAMING-AWS4-HMAC-SHA256-PAYLOAD") => Mode::StreamingSigned { trailers: false },
            Some("STREAMING-AWS4-HMAC-SHA256-PAYLOAD-TRAILER") => {
                Mode::StreamingSigned { trailers: true }
            }
            Some(v) if v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()) => {
                if presigned_s3 {
                    Mode::Unsigned
                } else {
                    Mode::Hashed
                }
            }
            None if presigned_s3 => Mode::Unsigned,
            None => Mode::Hashed,
            _ => return Err(StreamAuthenticationError::Unsupported),
        };
        let chunked = header(parts, "content-encoding")?.is_some_and(|v| {
            v.split(',')
                .any(|token| token.trim().eq_ignore_ascii_case("aws-chunked"))
        });
        if (mode.signed_chunks() || mode.requires_trailers()) && !chunked {
            return Err(StreamAuthenticationError::Unsupported);
        }
        // HTTP trailer-frame semantics are not AWS's embedded trailing-header representation.
        if header(parts, "trailer")?.is_some() {
            return Err(StreamAuthenticationError::Unsupported);
        }
        let declared = header(parts, "x-amz-trailer")?.unwrap_or("");
        if declared.len() > MAX_TRAILER_BYTES {
            return Err(StreamAuthenticationError::Unsupported);
        }
        let mut names = Vec::new();
        if !declared.is_empty() {
            for name in declared.split(',').map(|v| v.trim().to_ascii_lowercase()) {
                if !CHECKSUMS.iter().any(|(known, _)| *known == name) || names.contains(&name) {
                    return Err(StreamAuthenticationError::Unsupported);
                }
                names.push(name);
            }
        }
        if (mode.requires_trailers() && names.is_empty()) || (!chunked && !names.is_empty()) {
            return Err(StreamAuthenticationError::Unsupported);
        }
        for name in parts.headers.keys() {
            if name.as_str().starts_with("x-amz-checksum-")
                && !matches!(name.as_str(), "x-amz-checksum-mode" | "x-amz-checksum-type")
                && !CHECKSUMS.iter().any(|(known, _)| *known == name.as_str())
            {
                return Err(StreamAuthenticationError::Unsupported);
            }
        }
        let mut checksums = Vec::new();
        for (name, algorithm) in CHECKSUMS {
            let raw = header(parts, name)?;
            let trailer = names.iter().any(|v| v == name);
            if raw.is_some() && trailer {
                return Err(StreamAuthenticationError::Unsupported);
            }
            if raw.is_some() || trailer {
                let parsed: ChecksumAlgorithm = algorithm
                    .parse()
                    .map_err(|_| StreamAuthenticationError::Unsupported)?;
                let calculator = parsed.into_impl();
                let expected = raw
                    .map(|v| {
                        aws_smithy_types::base64::decode(v.trim())
                            .map_err(|_| StreamAuthenticationError::Unsupported)
                    })
                    .transpose()?;
                if expected.as_ref().is_some_and(|value| {
                    value.len() as u64 != aws_smithy_checksums::Checksum::size(&*calculator)
                }) {
                    return Err(StreamAuthenticationError::Unsupported);
                }
                checksums.push(ChecksumCheck {
                    name,
                    expected,
                    trailer,
                    calculator,
                });
            }
        }
        let expected_entity = length(parts, "content-length")?;
        let expected_decoded = length(parts, "x-amz-decoded-content-length")?;
        Ok(Self {
            parts: owned_parts(parts),
            credentials: None,
            chain: None,
            mode,
            state: if chunked { State::Header } else { State::Plain },
            failed: false,
            line: Vec::new(),
            chunk_signature: None,
            chunk_hash: hash(),
            entity_hash: hash(),
            decoded_hash: hash(),
            entity_bytes: 0,
            decoded_bytes: 0,
            expected_entity,
            expected_decoded,
            checksums,
            trailers: Vec::new(),
            trailer_bytes: 0,
        })
    }

    fn consume_inner<'a>(
        &mut self,
        input: &'a [u8],
    ) -> Result<PayloadStep<'a>, StreamValidationError> {
        if self.failed {
            return Err(framing("stream already failed"));
        }
        if input.is_empty() {
            return Ok(PayloadStep {
                consumed: 0,
                payload: None,
            });
        }
        let (consumed, payload) = match self.state {
            State::Plain => (input.len(), Some(input)),
            State::Complete => return Err(framing("bytes after terminal framing")),
            State::Data(remaining) => {
                let amount = remaining.min(input.len() as u64) as usize;
                let data = &input[..amount];
                self.chunk_hash.update(data);
                self.state = if remaining == amount as u64 {
                    State::DataCrlf(0)
                } else {
                    State::Data(remaining - amount as u64)
                };
                (amount, Some(data))
            }
            State::DataCrlf(matched) => {
                if input[0] != b"\r\n"[matched as usize] {
                    return Err(framing("missing chunk data terminator"));
                }
                if matched == 0 {
                    self.state = State::DataCrlf(1);
                } else {
                    self.finish_chunk()?;
                    self.state = State::Header;
                }
                (1, None)
            }
            State::Header | State::Trailers => {
                let is_trailer = matches!(self.state, State::Trailers);
                let mut n = 0;
                for byte in input {
                    n += 1;
                    if self.line.len() == MAX_LINE_BYTES {
                        return Err(StreamValidationError::LimitExceeded);
                    }
                    self.line.push(*byte);
                    if is_trailer {
                        self.trailer_bytes += 1;
                        if self.trailer_bytes > MAX_TRAILER_BYTES {
                            return Err(StreamValidationError::LimitExceeded);
                        }
                    }
                    if *byte == b'\n' {
                        if !self.line.ends_with(b"\r\n") {
                            return Err(framing("line is not CRLF terminated"));
                        }
                        self.line.truncate(self.line.len() - 2);
                        let line = std::mem::take(&mut self.line);
                        if is_trailer {
                            self.trailer_line(&line)?;
                        } else {
                            self.chunk_header(&line)?;
                        }
                        break;
                    }
                }
                (n, None)
            }
        };
        self.entity_bytes = self
            .entity_bytes
            .checked_add(consumed as u64)
            .ok_or(StreamValidationError::LimitExceeded)?;
        if self.expected_entity.is_some_and(|n| self.entity_bytes > n) {
            return Err(framing("entity exceeds content length"));
        }
        self.entity_hash.update(&input[..consumed]);
        if let Some(data) = payload {
            self.decoded_bytes = self
                .decoded_bytes
                .checked_add(data.len() as u64)
                .ok_or(StreamValidationError::LimitExceeded)?;
            if self
                .expected_decoded
                .is_some_and(|n| self.decoded_bytes > n)
            {
                return Err(integrity(StreamIntegrityFailure::DecodedLengthMismatch));
            }
            self.decoded_hash.update(data);
            for check in &mut self.checksums {
                check.calculator.update(data);
            }
        }
        Ok(PayloadStep { consumed, payload })
    }

    fn chunk_header(&mut self, line: &[u8]) -> Result<(), StreamValidationError> {
        let text = std::str::from_utf8(line).map_err(|_| framing("non-ASCII chunk header"))?;
        let mut fields = text.split(';');
        let size = fields.next().ok_or(framing("missing chunk size"))?;
        if size.is_empty() || !size.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(framing("invalid chunk size"));
        }
        let size = u64::from_str_radix(size, 16).map_err(|_| framing("chunk size overflow"))?;
        let mut signature = None;
        for field in fields {
            let value = field
                .strip_prefix("chunk-signature=")
                .ok_or(framing("unsupported chunk extension"))?;
            if signature.is_some()
                || value.len() != 64
                || !value.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err(framing("invalid chunk signature"));
            }
            signature = Some(value.to_owned());
        }
        if self.mode.signed_chunks() != signature.is_some() {
            return Err(framing("chunk signature does not match selected mode"));
        }
        self.chunk_signature = signature;
        self.chunk_hash = hash();
        if size == 0 {
            self.finish_chunk()?;
            self.state = State::Trailers;
        } else {
            self.state = State::Data(size);
        }
        Ok(())
    }

    fn finish_chunk(&mut self) -> Result<(), StreamValidationError> {
        if let Some(chain) = &mut self.chain {
            let digest = std::mem::replace(&mut self.chunk_hash, hash()).finish();
            let expected = chunk_signature_from_digest(
                &chain.signed,
                &chain.secret,
                &chain.previous,
                digest.as_ref(),
            )
            .ok_or(StreamValidationError::Authentication(ACCESS_DENIED))?;
            let signature = self
                .chunk_signature
                .take()
                .ok_or(framing("missing chunk signature"))?;
            if !same_secret(&expected, &signature) {
                return Err(StreamValidationError::Authentication(SIGNATURE_MISMATCH));
            }
            chain.previous = signature;
        }
        Ok(())
    }

    fn trailer_line(&mut self, line: &[u8]) -> Result<(), StreamValidationError> {
        if line.is_empty() {
            self.state = State::Complete;
            return Ok(());
        }
        if self.trailers.len() == MAX_TRAILERS {
            return Err(StreamValidationError::LimitExceeded);
        }
        let line = std::str::from_utf8(line).map_err(|_| framing("invalid trailer text"))?;
        let (name, value) = line.split_once(':').ok_or(framing("malformed trailer"))?;
        let name = name.to_ascii_lowercase();
        let permitted = self.checksums.iter().any(|c| c.trailer && c.name == name)
            || (self.mode.signed_trailers() && name == TRAILER_SIGNATURE);
        if !permitted || self.trailers.iter().any(|(existing, _)| existing == &name) {
            return Err(integrity(
                StreamIntegrityFailure::TrailerDeclarationMismatch,
            ));
        }
        if value.bytes().any(|b| b < 0x20 && b != b'\t') || value.bytes().any(|b| b == 0x7f) {
            return Err(framing("invalid trailer value"));
        }
        self.trailers.push((name, value.trim().into()));
        Ok(())
    }

    fn finish_inner(
        self,
        http_trailers: Option<&http::HeaderMap>,
    ) -> Result<StreamAuthentication, StreamValidationError> {
        if self.failed {
            return Err(framing("stream already failed"));
        }
        if !matches!(self.state, State::Plain | State::Complete) || !self.line.is_empty() {
            return Err(framing("truncated AWS chunk stream"));
        }
        if http_trailers.is_some_and(|t| !t.is_empty()) {
            return Err(framing("HTTP trailer frames are not AWS embedded trailers"));
        }
        if self.expected_entity.is_some_and(|n| n != self.entity_bytes) {
            return Err(framing("entity content length mismatch"));
        }
        if self
            .expected_decoded
            .is_some_and(|n| n != self.decoded_bytes)
        {
            return Err(integrity(StreamIntegrityFailure::DecodedLengthMismatch));
        }
        let mut validated_checksums = Vec::new();
        for check in self.checksums {
            let expected = if check.trailer {
                let value = self
                    .trailers
                    .iter()
                    .find(|(name, _)| name == check.name)
                    .ok_or(integrity(
                        StreamIntegrityFailure::TrailerDeclarationMismatch,
                    ))?;
                aws_smithy_types::base64::decode(&value.1)
                    .map_err(|_| integrity(StreamIntegrityFailure::MalformedChecksum))?
            } else {
                check
                    .expected
                    .ok_or(integrity(StreamIntegrityFailure::MalformedChecksum))?
            };
            let actual = check.calculator.finalize();
            if actual.len() != expected.len() {
                return Err(integrity(StreamIntegrityFailure::MalformedChecksum));
            }
            if actual.as_ref() != expected.as_slice() {
                return Err(integrity(StreamIntegrityFailure::ChecksumMismatch));
            }
            validated_checksums.push((
                check.name,
                aws_smithy_types::base64::encode(actual.as_ref()),
            ));
        }
        if self.mode.signed_trailers() {
            let chain = self
                .chain
                .as_ref()
                .ok_or(StreamValidationError::Authentication(ACCESS_DENIED))?;
            let supplied = self
                .trailers
                .iter()
                .find(|(name, _)| name == TRAILER_SIGNATURE)
                .ok_or(integrity(
                    StreamIntegrityFailure::TrailerDeclarationMismatch,
                ))?;
            let mut headers = aws_smithy_runtime_api::http::Headers::new();
            for (name, value) in &self.trailers {
                if name != TRAILER_SIGNATURE {
                    headers
                        .try_insert(name.clone(), value.clone())
                        .map_err(|_| framing("invalid signed trailer"))?;
                }
            }
            let expected = with_signing_params(&chain.signed, &chain.secret, (), |params| {
                aws_sigv4::sign::v4::sign_trailer(&headers, &chain.previous, &params)
                    .ok()
                    .map(|s| s.signature().to_owned())
            })
            .ok_or(StreamValidationError::Authentication(ACCESS_DENIED))?;
            if !same_secret(&expected, &supplied.1) {
                return Err(StreamValidationError::Authentication(SIGNATURE_MISMATCH));
            }
        }
        let entity_digest = self.entity_hash.finish();
        let binding = if let Some(credentials) = &self.credentials {
            let (signed, secret) = checked_signature_credentials(
                credentials.authenticator,
                &self.parts,
                &credentials.serving,
                &credentials.store,
            )
            .map_err(StreamValidationError::Authentication)?;
            let payload = match self.mode.marker() {
                Some(marker) => SignableBody::Precomputed(marker.into()),
                None => SignableBody::Precomputed(hex(entity_digest.as_ref())),
            };
            let expected = signature_for_payload(&self.parts, &signed, &secret, payload);
            if !expected.is_some_and(|value| same_secret(&value, &signed.value)) {
                return Err(StreamValidationError::Authentication(SIGNATURE_MISMATCH));
            }
            if self.mode.signed_chunks() {
                PayloadSignatureBinding::SignedChunks
            } else if matches!(self.mode, Mode::Hashed) {
                PayloadSignatureBinding::Sha256
            } else {
                PayloadSignatureBinding::Unsigned
            }
        } else {
            PayloadSignatureBinding::Unsigned
        };
        let mut decoded_sha256 = [0; 32];
        decoded_sha256.copy_from_slice(self.decoded_hash.finish().as_ref());
        Ok(StreamAuthentication {
            entity_bytes: self.entity_bytes,
            decoded_bytes: self.decoded_bytes,
            decoded_sha256,
            binding,
            checksums: validated_checksums,
        })
    }
}

impl RequestAuthenticationStream for Stream {
    fn consume<'a>(&mut self, input: &'a [u8]) -> Result<PayloadStep<'a>, StreamValidationError> {
        let result = self.consume_inner(input);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn finish(
        self: Box<Self>,
        trailers: Option<&http::HeaderMap>,
    ) -> Result<StreamAuthentication, StreamValidationError> {
        (*self).finish_inner(trailers)
    }
}

// SDK sign_chunk hashes a whole Bytes first. Only that digest input is replaced here;
// key derivation and HMAC stay SDK-owned. Differential tests use native sign_chunk itself.
fn chunk_signature_from_digest(
    signed: &Signature,
    secret: &str,
    previous: &str,
    digest: &[u8],
) -> Option<String> {
    let time = parse_amz_date(&signed.amz_date)?;
    let [_, _, region, service, _] = &signed.scope[..] else {
        return None;
    };
    let at = time::OffsetDateTime::from(time);
    let date = format!("{:04}{:02}{:02}", at.year(), u8::from(at.month()), at.day());
    let timestamp = format!(
        "{date}T{:02}{:02}{:02}Z",
        at.hour(),
        at.minute(),
        at.second()
    );
    let scope = format!("{date}/{region}/{service}/aws4_request");
    let empty = digest::digest(&digest::SHA256, &[]);
    let text = format!(
        "AWS4-HMAC-SHA256-PAYLOAD\n{timestamp}\n{scope}\n{previous}\n{}\n{}",
        hex(empty.as_ref()),
        hex(digest)
    );
    let key = aws_sigv4::sign::v4::generate_signing_key(secret, time, region, service);
    Some(aws_sigv4::sign::v4::calculate_signature(
        key,
        text.as_bytes(),
    ))
}

#[cfg(test)]
mod tests;
