// SPDX-License-Identifier: CC0-1.0

//! The RESPONSE COMPRESSION an api asks for with `minimumCompressionSize`: which content
//! coding a response is served under, and the bytes that coding produces.
//!
//! `RestApi$minimumCompressionSize` is "a nullable integer that is used to enable
//! compression (with non-negative between 0 and 10485760 (10M) bytes, inclusive) or disable
//! compression (with a null value) on an API", and API Gateway "does not apply compression
//! or decompression on the payload if the payload size is smaller than the configured
//! threshold". So a response is compressed when all of these hold:
//!
//! 1. the api carries a `minimumCompressionSize` — a null one is compression DISABLED, and
//!    nothing about the response changes;
//! 2. the body is at least that many bytes;
//! 3. the client asked for a coding this endpoint serves ([`negotiated`]): API Gateway
//!    documents `gzip`, `deflate` and `identity` as the supported content encodings, and
//!    `identity` is the bytes as they are;
//! 4. the response is not already encoded — one that declares a `Content-Encoding` is
//!    served with the bytes and the coding it already carries.
//!
//! The compression is real: the bytes the client receives are the bytes the named encoder
//! produced from the response's own, and the `Content-Encoding` the response carries is
//! what decodes them. Nothing is relabelled.

use std::io::Write;

use super::*;

/// A content coding this endpoint compresses a response with: the two
/// `Accept-Encoding` codings API Gateway's compression documents beside `identity`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Encoding {
    /// `gzip`, the gzip framing of the DEFLATE stream (RFC 1952).
    Gzip,
    /// `deflate`, which on the wire is the zlib framing of that stream (RFC 1950), as
    /// HTTP's own content-coding registry defines the token.
    Deflate,
}

/// How well the bytes are compressed: the middle of the encoder's range, so one request
/// cannot ask this process for the work the highest setting does. The same setting the
/// sibling edge service compresses an answer at.
const COMPRESSION_QUALITY: u32 = 5;

/// The largest body this endpoint compresses: the payload limit of a request to a REST API,
/// which is the largest body any integration of it can have produced. A larger body is
/// served exactly as the integration answered, so one request cannot ask this process for
/// unbounded compression work.
const MAX_COMPRESSED_BYTES: usize = MAX_REQUEST_PAYLOAD_BYTES;

impl Encoding {
    /// The `Accept-Encoding` / `Content-Encoding` token this coding travels under.
    pub(super) fn token(self) -> &'static str {
        match self {
            Self::Gzip => "gzip",
            Self::Deflate => "deflate",
        }
    }

    /// `bytes` as this coding encodes them, or `None` when the encoder did not produce
    /// them — in which case the caller serves the body as it was, uncompressed, rather than
    /// labelling bytes with a coding that does not decode them.
    pub(super) fn compress(self, bytes: &[u8]) -> Option<Vec<u8>> {
        if bytes.len() > MAX_COMPRESSED_BYTES {
            return None;
        }
        let quality = ::flate2::Compression::new(COMPRESSION_QUALITY);
        match self {
            Self::Gzip => {
                let mut encoder = ::flate2::write::GzEncoder::new(Vec::new(), quality);
                encoder.write_all(bytes).ok()?;
                encoder.finish().ok()
            }
            Self::Deflate => {
                let mut encoder = ::flate2::write::ZlibEncoder::new(Vec::new(), quality);
                encoder.write_all(bytes).ok()?;
                encoder.finish().ok()
            }
        }
    }
}

/// The request header a client names the codings it accepts in.
const ACCEPT_ENCODING_HEADER: &str = "accept-encoding";

/// The response header that says which coding the body is under.
const CONTENT_ENCODING_HEADER: &str = "content-encoding";

/// The coding `accepted` asks for, `gzip` first, or `None` for a client that asked for
/// none this endpoint serves.
///
/// An `Accept-Encoding` value is a comma-separated list of codings, each optionally with a
/// `q` weight. A coding offered with `q=0` is one the client REFUSES, and nothing re-enables
/// it: `*` is the wildcard RFC 9110 gives for "any available content coding", so a client
/// that sends it accepts whatever this endpoint serves OTHER than the codings it refused by
/// name. `identity` names the bytes as they are and so is never a coding to compress with.
pub(super) fn negotiated(accepted: Option<&str>) -> Option<Encoding> {
    // The two lists are kept apart, because a refusal is not the absence of an offer: an
    // explicitly refused coding stays refused however the rest of the value reads.
    let mut offered: Vec<&str> = Vec::new();
    let mut refused: Vec<&str> = Vec::new();
    for coding in accepted?.split(',') {
        let (token, parameters) = coding.split_once(';').unwrap_or((coding, ""));
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        let weighted_zero = parameters.split(';').any(|parameter| {
            let (name, value) = parameter.split_once('=').unwrap_or((parameter, ""));
            name.trim().eq_ignore_ascii_case("q") && value.trim().parse::<f32>() == Ok(0.0)
        });
        if weighted_zero {
            refused.push(token);
        } else {
            offered.push(token);
        }
    }
    let wildcard = offered.contains(&ANY_CODING);
    // gzip first: it is the coding a client that offers both expects, and the one API
    // Gateway's own documentation names first.
    [Encoding::Gzip, Encoding::Deflate]
        .into_iter()
        .find(|encoding| {
            let named_in = |list: &[&str]| {
                list.iter()
                    .any(|token| token.eq_ignore_ascii_case(encoding.token()))
            };
            !named_in(&refused) && (named_in(&offered) || wildcard)
        })
}

/// The `Accept-Encoding` wildcard: "any available content coding" (RFC 9110), which a client
/// with no preference of its own sends to take whatever the endpoint has.
const ANY_CODING: &str = "*";

/// `response`, compressed as the api that served it asks for.
///
/// Applied at the ONE funnel every outcome of this data plane reaches ([`super::serve`]), so
/// a served integration and a gateway refusal are compressed under one rule — and the
/// STAGE's cache holds the integration's own uncompressed answer, which is what lets a
/// cached response be served to a client that accepts no coding at all.
pub(super) fn compressed<Eng: Send + Sync + 'static>(
    engine: &Eng,
    req: &RtRequest,
    response: RtResponse,
) -> RtResponse {
    let Some(threshold) = minimum_compression_size(engine, req) else {
        return response;
    };
    if response.headers().get(CONTENT_ENCODING_HEADER).is_some() {
        return response;
    }
    let Some(encoding) = negotiated(req.headers().get(ACCEPT_ENCODING_HEADER)) else {
        return response;
    };
    let Some(body) = response.body().bytes().filter(|body| body.len() >= threshold) else {
        return response;
    };
    let Some(compressed) = encoding.compress(body) else {
        return response;
    };
    let mut response = response;
    let length = compressed.len();
    *response.body_mut() = SdkBody::from(compressed);
    let _ = response
        .headers_mut()
        .try_insert(CONTENT_ENCODING_HEADER, encoding.token());
    let _ = response
        .headers_mut()
        .try_insert("content-length", length.to_string());
    response
}

/// The `minimumCompressionSize` of the api this request addressed, as a byte count, or
/// `None` when the endpoint serves no api of this account or the api's compression is
/// disabled.
///
/// A negative stored value is no byte count and is read as compression disabled: the member
/// is "non-negative between 0 and 10485760", so nothing within the documented range is
/// lost.
fn minimum_compression_size<Eng: Send + Sync + 'static>(
    engine: &Eng,
    req: &RtRequest,
) -> Option<usize> {
    let state = crate::state::service_state(engine)?;
    let store = state.lock();
    let addressed = Addressed::of_request(req);
    let api = resolve(&store, &addressed)?.api;
    usize::try_from(api.minimum_compression_size?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_coding_a_client_accepts_is_the_one_it_is_served_under() {
        // Rows differ along the `Accept-Encoding` value: one coding, several, the order
        // reversed, a weight, a refusal weight, `identity`, a coding this endpoint does not
        // serve, and no header at all.
        // Rows differ along the `Accept-Encoding` value: one coding, several, the order
        // reversed, a weight, a refusal weight, a refusal BESIDE the wildcard (which does
        // not re-enable it), `identity`, a coding this endpoint does not serve, and no
        // header at all.
        let cases: [(&str, Option<&str>, Option<Encoding>); 16] = [
            ("gzip alone", Some("gzip"), Some(Encoding::Gzip)),
            ("deflate alone", Some("deflate"), Some(Encoding::Deflate)),
            ("both, gzip first", Some("gzip, deflate"), Some(Encoding::Gzip)),
            ("both, deflate first", Some("deflate, gzip"), Some(Encoding::Gzip)),
            ("gzip with a weight", Some("gzip;q=0.5"), Some(Encoding::Gzip)),
            ("gzip refused, deflate offered", Some("gzip;q=0, deflate"), Some(Encoding::Deflate)),
            ("the wildcard alone", Some("*"), Some(Encoding::Gzip)),
            ("the wildcard with a weight", Some("*;q=1"), Some(Encoding::Gzip)),
            ("the wildcard refused", Some("*;q=0"), None),
            // A coding refused by NAME stays refused beside the wildcard: RFC 9110's `*`
            // stands for the codings the value does not name, not for the ones it rejects.
            ("gzip refused beside the wildcard", Some("gzip;q=0, *"), Some(Encoding::Deflate)),
            ("deflate refused beside the wildcard", Some("deflate;q=0, *"), Some(Encoding::Gzip)),
            ("both refused beside the wildcard", Some("gzip;q=0, deflate;q=0, *"), None),
            ("identity only", Some("identity"), None),
            ("a coding this endpoint does not serve", Some("br"), None),
            ("an empty value", Some(""), None),
            ("no header at all", None, None),
        ];
        for (case, accepted, expected) in cases {
            let actual = negotiated(accepted);
            assert_eq!(
                actual, expected,
                "case {case:?} ({accepted:?}): expected {expected:?}, got {actual:?}"
            );
        }
    }

    #[test]
    fn a_compressed_body_is_the_encoders_own_bytes() {
        // Rows differ along the coding and along the bytes: text that compresses, an empty
        // body, and bytes that are not text.
        let text = r#"{"body":"simparity"}"#.repeat(16).into_bytes();
        let cases: [(&str, Encoding, Vec<u8>); 6] = [
            ("gzip over text", Encoding::Gzip, text.clone()),
            ("deflate over text", Encoding::Deflate, text.clone()),
            ("gzip over an empty body", Encoding::Gzip, Vec::new()),
            ("deflate over an empty body", Encoding::Deflate, Vec::new()),
            ("gzip over arbitrary bytes", Encoding::Gzip, (0u8..=255).collect()),
            ("deflate over arbitrary bytes", Encoding::Deflate, (0u8..=255).collect()),
        ];
        for (case, encoding, bytes) in cases {
            let compressed = encoding
                .compress(&bytes)
                .unwrap_or_else(|| panic!("case {case:?}: the encoder produced no bytes"));
            let decoded = match encoding {
                Encoding::Gzip => {
                    let mut decoder = ::flate2::read::GzDecoder::new(&compressed[..]);
                    let mut decoded = Vec::new();
                    ::std::io::Read::read_to_end(&mut decoder, &mut decoded).unwrap_or_else(
                        |error| panic!("case {case:?}: the compressed bytes do not decode: {error}"),
                    );
                    decoded
                }
                Encoding::Deflate => {
                    let mut decoder = ::flate2::read::ZlibDecoder::new(&compressed[..]);
                    let mut decoded = Vec::new();
                    ::std::io::Read::read_to_end(&mut decoder, &mut decoded).unwrap_or_else(
                        |error| panic!("case {case:?}: the compressed bytes do not decode: {error}"),
                    );
                    decoded
                }
            };
            assert_eq!(
                decoded,
                bytes,
                "case {case:?}: the compressed bytes decode to {} bytes, not the body's {}",
                decoded.len(),
                bytes.len()
            );
        }
    }
}
