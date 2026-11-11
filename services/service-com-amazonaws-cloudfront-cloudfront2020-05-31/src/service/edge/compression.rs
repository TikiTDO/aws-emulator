// SPDX-License-Identifier: CC0-1.0

//! The AUTOMATIC COMPRESSION a cache behaviour asks for: which encoding an answer is served
//! under, and the bytes that encoding produces.
//!
//! `CacheBehavior$Compress` is "whether you want CloudFront to automatically compress
//! certain files for this cache behavior", and the behaviour's cache policy decides which
//! encodings are negotiated at all (`EnableAcceptEncodingGzip` /
//! `EnableAcceptEncodingBrotli`, read in [`super::forwarding`]). So an answer is compressed
//! when all of these hold:
//!
//! 1. the behaviour asks for compression (`Compress`);
//! 2. the viewer accepts an encoding the behaviour's cache policy enables, which is what the
//!    normalized `Accept-Encoding` names ([`super::forwarding::AcceptEncoding`]) — Brotli
//!    ahead of gzip, as CloudFront prefers it;
//! 3. the origin's own answer is not already encoded: a response that declares a
//!    `Content-Encoding` is served with the bytes and the encoding the origin produced;
//! 4. the answer's media type is one of "certain files" — text and the structured text
//!    formats; a response that declares an image, a video or an archive is served as the
//!    origin sent it ([`compressible`]).
//!
//! The compression is real: the bytes the viewer receives are the bytes the named encoder
//! produced from the origin's own, and the `Content-Encoding` the answer carries is what
//! decodes them. Nothing is relabelled.

use std::io::Write;

/// A content coding the edge compresses an answer with.
///
/// The two `Accept-Encoding` codings a cache policy can negotiate: the model's
/// `EnableAcceptEncodingBrotli` and `EnableAcceptEncodingGzip` name no others.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    /// `br`, the coding `EnableAcceptEncodingBrotli` negotiates.
    Brotli,
    /// `gzip`, the coding `EnableAcceptEncodingGzip` negotiates.
    Gzip,
}

/// How well the bytes are compressed: the middle of each encoder's own range, so one viewer
/// request cannot ask this process for the work the highest setting does.
const COMPRESSION_QUALITY: u32 = 5;

/// The brotli window size, in bits — the encoder's own default (`lgwin` 22).
const BROTLI_WINDOW_BITS: u32 = 22;

impl Encoding {
    /// The `Accept-Encoding` / `Content-Encoding` token this coding travels under.
    pub fn token(self) -> &'static str {
        match self {
            Self::Brotli => "br",
            Self::Gzip => "gzip",
        }
    }

    /// `bytes` as this coding encodes them, or `None` when the encoder did not produce them —
    /// in which case the caller serves the origin's own bytes, uncompressed, rather than
    /// labelling bytes with an encoding that does not decode them.
    ///
    /// Bytes past [`MAX_COMPRESSED_BYTES`] are not compressed at all, so the work this
    /// answers for is bounded wherever it is called from.
    pub fn compress(self, bytes: &[u8]) -> Option<Vec<u8>> {
        if bytes.len() > MAX_COMPRESSED_BYTES {
            return None;
        }
        match self {
            Self::Brotli => {
                let mut encoded = Vec::new();
                ::brotli::BrotliCompress(
                    &mut ::std::io::Cursor::new(bytes),
                    &mut encoded,
                    &::brotli::enc::BrotliEncoderParams {
                        quality: i32::try_from(COMPRESSION_QUALITY).unwrap_or(5),
                        lgwin: i32::try_from(BROTLI_WINDOW_BITS).unwrap_or(22),
                        ..Default::default()
                    },
                )
                .ok()?;
                Some(encoded)
            }
            Self::Gzip => {
                let mut encoder = ::flate2::write::GzEncoder::new(
                    Vec::new(),
                    ::flate2::Compression::new(COMPRESSION_QUALITY),
                );
                encoder.write_all(bytes).ok()?;
                encoder.finish().ok()
            }
        }
    }
}

/// The media types the edge compresses: text, and the structured formats that are text.
///
/// The list CloudFront publishes for automatic compression ("Serving compressed files", the
/// file types it compresses) is text and the `application/*` formats that carry text; this
/// carries the type families rather than a frozen list of subtypes, so a text format the
/// list gained is compressed too.
const COMPRESSIBLE_TYPES: [&str; 6] = [
    "text/",
    "application/json",
    "application/javascript",
    "application/xml",
    "application/x-javascript",
    "image/svg+xml",
];

/// The largest answer the edge compresses.
///
/// The maximum size CloudFront's own automatic compression documents for the files it
/// compresses ("Serving compressed files": the file size must be at most 10,000,000 bytes).
/// A larger answer is served exactly as the origin sent it, so one viewer request cannot ask
/// this process for more compression work than that.
pub const MAX_COMPRESSED_BYTES: usize = 10_000_000;

/// Whether an answer of `content_type` is one of the "certain files" the edge compresses.
///
/// A response that declares a media type outside the published list — an image, a video, an
/// archive, bytes of no stated structure — is already compressed or does not compress, so it
/// is served as the origin sent it. A response that declares NO media type is compressed:
/// the viewer asked for the encoding, and nothing says those bytes are already compressed.
pub fn compressible(content_type: Option<&str>) -> bool {
    let Some(content_type) = content_type else {
        return true;
    };
    let media_type = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase();
    COMPRESSIBLE_TYPES
        .iter()
        .any(|family| match family.strip_suffix('/') {
            // A family: every media type UNDER it, the `/` included, so a type that merely
            // begins with the family's letters is not one of its members.
            Some(_) => media_type.starts_with(family),
            None => media_type == *family,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_compressed_answer_is_the_encoders_own_bytes() {
        // Rows differ along the encoding and along the bytes: text that compresses, an empty
        // body, and bytes that are not text.
        let text = "{\"marker\":\"simparity\",\"path\":\"/br/obj\"}".repeat(8);
        let cases: [(&str, Encoding, Vec<u8>); 6] = [
            ("brotli over text", Encoding::Brotli, text.clone().into_bytes()),
            ("gzip over text", Encoding::Gzip, text.clone().into_bytes()),
            ("brotli over an empty body", Encoding::Brotli, Vec::new()),
            ("gzip over an empty body", Encoding::Gzip, Vec::new()),
            ("brotli over arbitrary bytes", Encoding::Brotli, (0u8..=255).collect()),
            ("gzip over arbitrary bytes", Encoding::Gzip, (0u8..=255).collect()),
        ];
        for (case, encoding, bytes) in cases {
            let compressed = encoding
                .compress(&bytes)
                .unwrap_or_else(|| panic!("case {case:?}: the encoder produced no bytes"));
            // The bytes really are the coding's: decoding them gives the original back.
            let decoded = match encoding {
                Encoding::Brotli => {
                    let mut decoded = Vec::new();
                    ::brotli::BrotliDecompress(
                        &mut ::std::io::Cursor::new(&compressed),
                        &mut decoded,
                    )
                    .unwrap_or_else(|error| {
                        panic!("case {case:?}: the compressed bytes do not decode: {error}")
                    });
                    decoded
                }
                Encoding::Gzip => {
                    let mut decoder = ::flate2::read::GzDecoder::new(&compressed[..]);
                    let mut decoded = Vec::new();
                    ::std::io::Read::read_to_end(&mut decoder, &mut decoded).unwrap_or_else(
                        |error| {
                            panic!("case {case:?}: the compressed bytes do not decode: {error}")
                        },
                    );
                    decoded
                }
            };
            assert_eq!(
                decoded, bytes,
                "case {case:?}: the compressed bytes decode to {} bytes, not the origin's {}",
                decoded.len(),
                bytes.len()
            );
        }
    }

    #[test]
    fn only_a_text_media_type_is_compressed() {
        // Rows differ along the media type: text, the structured text formats, one with a
        // charset parameter, an image, a binary stream, a type that merely begins with a
        // family's letters, and a response declaring none.
        let cases: [(&str, Option<&str>, bool); 10] = [
            ("html", Some("text/html"), true),
            ("css with a charset", Some("text/css; charset=utf-8"), true),
            ("json", Some("application/json"), true),
            ("xml, as a control plane answers", Some("text/xml"), true),
            ("an SVG image, which is text", Some("image/svg+xml"), true),
            ("a PNG image", Some("image/png"), false),
            ("a video", Some("video/mp4"), false),
            ("bytes of no stated structure", Some("application/octet-stream"), false),
            ("a type that only begins with a family's letters", Some("texture/x-foo"), false),
            ("a response declaring no media type", None, true),
        ];
        for (case, content_type, expected) in cases {
            let actual = compressible(content_type);
            assert_eq!(
                actual, expected,
                "case {case:?} ({content_type:?}): expected compressible == {expected}, got \
                 {actual}"
            );
        }
    }
}
