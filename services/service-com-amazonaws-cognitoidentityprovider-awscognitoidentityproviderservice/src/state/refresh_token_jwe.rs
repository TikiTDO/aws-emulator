// SPDX-License-Identifier: CC0-1.0

//! The FORM a refresh token travels in: the JWE compact serialization of RFC 7516 §7.1,
//! and the key this service seals one under.
//!
//! A refresh token is the one token of a sign-in that states nothing to its holder — it is
//! a bearer credential, and everything this service knows about it lives in the record it
//! keyed by it. What it is not is an undelimited run of characters: Amazon Cognito vends it
//! as a JOSE value in COMPACT serialization — `.`-separated base64url segments whose first
//! one decodes to a JOSE header — which is what the `.` in the modelled `TokenModelType`
//! pattern `^[A-Za-z0-9-_=.]+$` admits, and what any client, proxy or test that reads the
//! token's structure before presenting it back sees. Sealing the credential inside is what
//! keeps the token opaque to its holder while its form stays the published one.
//!
//! One pass of RFC 7516 §5.1, with the two algorithms the protected header names:
//!
//! * the content encryption key is sealed under this service's own RSA key with RSAES-OAEP
//!   (`alg` [`KEY_MANAGEMENT_ALGORITHM`], RFC 7518 §4.3) and travels in the second
//!   segment;
//! * the credential is encrypted under that key with AES-256-GCM (`enc`
//!   [`CONTENT_ENCRYPTION_ALGORITHM`], RFC 7518 §5.3), with the ENCODED protected header as
//!   the additional authenticated data RFC 7516 §5.1 step 14 requires, and travels as the
//!   initialization vector, the ciphertext and the authentication tag of the last three
//!   segments.
//!
//! The key is this SERVICE's and not any pool's: a pool publishes the keys its identity and
//! access tokens are SIGNED with, in the key set it serves at `jwks.json`, and publishes no
//! key a refresh token is sealed under, because nobody but the service that issued one ever
//! opens it. It is generated once, on the first sign-in that vends a refresh token, and is
//! never put in a response, a log or a published document.
//!
//! Nothing cryptographic is composed here. The key generation and the RSAES-OAEP pass come
//! from `aws-lc-rs` (`aws_lc_rs::rsa`), the content encryption and the randomness from
//! `ring` (`ring::aead::AES_256_GCM`, `ring::rand::SystemRandom`) — the two libraries this
//! crate already generates its pool keys and encrypts its custom-sender messages with.
//! [`RefreshTokenKey::seal`] and [`RefreshTokenKey::open`] are each other's inverse, which
//! is what the round-trip test below pins.

use super::*;

use ::ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use ::ring::rand::{SecureRandom, SystemRandom};

/// The `alg` of every refresh token this service seals: RSAES-OAEP with SHA-256 as both
/// the message digest and the hash of MGF1, which is the registered JOSE name for that
/// parameter set (RFC 7518 §4.3).
pub const KEY_MANAGEMENT_ALGORITHM: &str = "RSA-OAEP-256";

/// The `enc` of every refresh token this service seals: AES-256-GCM (RFC 7518 §5.3).
pub const CONTENT_ENCRYPTION_ALGORITHM: &str = "A256GCM";

/// How many segments a JWE in compact serialization carries (RFC 7516 §7.1): the protected
/// header, the encrypted key, the initialization vector, the ciphertext and the
/// authentication tag.
pub const COMPACT_SEGMENTS: usize = 5;

/// The width of the content encryption key [`CONTENT_ENCRYPTION_ALGORITHM`] takes: 256
/// bits (RFC 7518 §5.3).
const CONTENT_KEY_BYTES: usize = 32;

/// The modulus the key-management key pair is generated at: 2048 bits, the width RSAES-OAEP
/// is used at here and the smallest a JOSE implementation accepts.
const KEY_MANAGEMENT_KEY_SIZE: ::aws_lc_rs::rsa::KeySize = ::aws_lc_rs::rsa::KeySize::Rsa2048;

/// The key every refresh token this emulator vends is sealed under.
///
/// The private half is held as PKCS#8 and parsed per use by the library that implements the
/// scheme, exactly as the key material of the other emulated services that seal a value is
/// — so this type carries no key handle and the state that holds it stays shareable.
#[derive(Clone)]
pub struct RefreshTokenKey {
    /// The key pair, PKCS#8. Private key material, so it is never rendered, logged or put
    /// in a response (see the [`::std::fmt::Debug`] impl below).
    private_key_pkcs8: Vec<u8>,
}

/// Renders the ALGORITHMS this key is used under and nothing else: private key material is
/// never logged or traced, so it is not in this rendering either.
impl ::std::fmt::Debug for RefreshTokenKey {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("RefreshTokenKey")
            .field("alg", &KEY_MANAGEMENT_ALGORITHM)
            .field("enc", &CONTENT_ENCRYPTION_ALGORITHM)
            .finish_non_exhaustive()
    }
}

impl RefreshTokenKey {
    /// A freshly generated key pair, drawn from `aws-lc-rs`'s own generator.
    ///
    /// INFALLIBLE: the only failures the generator and the encoder report are the library
    /// failing to produce or encode an RSA key pair, and this emulator vends no refresh
    /// token without one.
    #[must_use]
    pub fn generate() -> Self {
        use ::aws_lc_rs::encoding::AsDer as _;

        let generated = ::aws_lc_rs::rsa::PrivateDecryptingKey::generate(KEY_MANAGEMENT_KEY_SIZE)
            .expect("the library yields an RSA key pair");
        let private_key_pkcs8 = generated
            .as_der()
            .expect("a freshly generated RSA key pair encodes as PKCS#8")
            .as_ref()
            .to_vec();
        Self { private_key_pkcs8 }
    }

    /// `plaintext` as a JWE in compact serialization under this key, or `None` when the
    /// randomness or one of the two encryption passes failed — in which case the caller
    /// vends no credential rather than one that is not sealed.
    #[must_use]
    pub fn seal(&self, plaintext: &[u8]) -> Option<String> {
        let private_key =
            ::aws_lc_rs::rsa::PrivateDecryptingKey::from_pkcs8(&self.private_key_pkcs8).ok()?;
        let public_key = ::aws_lc_rs::rsa::OaepPublicEncryptingKey::new(private_key.public_key())
            .ok()?;

        let random = SystemRandom::new();
        let mut content_key = [0_u8; CONTENT_KEY_BYTES];
        let mut iv = [0_u8; NONCE_LEN];
        random.fill(&mut content_key).ok()?;
        random.fill(&mut iv).ok()?;

        // An RSA ciphertext is exactly one modulus wide.
        let mut encrypted_key = vec![0_u8; private_key.key_size_bytes()];
        let sealed_key_len = public_key
            .encrypt(
                &::aws_lc_rs::rsa::OAEP_SHA256_MGF1SHA256,
                &content_key,
                &mut encrypted_key,
                None,
            )
            .ok()?
            .len();
        encrypted_key.truncate(sealed_key_len);

        // RFC 7516 §5.1 step 14: the additional authenticated data of the content
        // encryption is the ENCODED protected header, so those exact bytes are what the
        // token's first segment carries and what opening it authenticates against.
        let protected = encoded_protected_header();
        let cipher = content_cipher(&content_key)?;
        let mut ciphertext = plaintext.to_vec();
        let tag = cipher
            .seal_in_place_separate_tag(
                Nonce::assume_unique_for_key(iv),
                Aad::from(protected.as_bytes()),
                &mut ciphertext,
            )
            .ok()?;

        Some(format!(
            "{protected}.{}.{}.{}.{}",
            base64_url(&encrypted_key),
            base64_url(&iv),
            base64_url(&ciphertext),
            base64_url(tag.as_ref())
        ))
    }

    /// The plaintext `token` carries, or `None` when `token` is not a JWE this key sealed:
    /// a value with the wrong number of segments, a header naming other algorithms, a
    /// content key no other key wrapped, or a ciphertext or header that was altered after
    /// it was sealed.
    #[must_use]
    pub fn open(&self, token: &str) -> Option<Vec<u8>> {
        let segments: Vec<&str> = token.split('.').collect();
        if segments.len() != COMPACT_SEGMENTS {
            return None;
        }
        let header: serde_json::Value =
            serde_json::from_slice(&base64_url_decode(segments[0])?).ok()?;
        if header.get("alg").and_then(serde_json::Value::as_str) != Some(KEY_MANAGEMENT_ALGORITHM)
            || header.get("enc").and_then(serde_json::Value::as_str)
                != Some(CONTENT_ENCRYPTION_ALGORITHM)
        {
            return None;
        }
        let encrypted_key = base64_url_decode(segments[1])?;
        let iv: [u8; NONCE_LEN] = base64_url_decode(segments[2])?.try_into().ok()?;
        // `ring` opens an AEAD message in place, over the ciphertext followed by its tag.
        let mut sealed = base64_url_decode(segments[3])?;
        sealed.extend_from_slice(&base64_url_decode(segments[4])?);

        let private_key =
            ::aws_lc_rs::rsa::PrivateDecryptingKey::from_pkcs8(&self.private_key_pkcs8).ok()?;
        // An OAEP plaintext is shorter than the modulus its ciphertext occupies, so one
        // modulus of room always holds it.
        let mut content_key = vec![0_u8; private_key.key_size_bytes()];
        let private_key = ::aws_lc_rs::rsa::OaepPrivateDecryptingKey::new(private_key).ok()?;
        let content_key_len = private_key
            .decrypt(
                &::aws_lc_rs::rsa::OAEP_SHA256_MGF1SHA256,
                &encrypted_key,
                &mut content_key,
                None,
            )
            .ok()?
            .len();
        content_key.truncate(content_key_len);

        let cipher = content_cipher(&content_key)?;
        let plaintext = cipher
            .open_in_place(
                Nonce::assume_unique_for_key(iv),
                Aad::from(segments[0].as_bytes()),
                &mut sealed,
            )
            .ok()?;
        Some(plaintext.to_vec())
    }
}

/// The protected header of every refresh token this service seals, base64url-encoded as
/// RFC 7516 §7.1 puts it in the first segment: the two algorithms and nothing else, since
/// one key seals them all and no party but this service opens one.
fn encoded_protected_header() -> String {
    base64_url(
        serde_json::json!({
            "alg": KEY_MANAGEMENT_ALGORITHM,
            "enc": CONTENT_ENCRYPTION_ALGORITHM,
        })
        .to_string()
        .as_bytes(),
    )
}

/// An AES-256-GCM key over `key`, or `None` when `key` is not of that key's width.
fn content_cipher(key: &[u8]) -> Option<LessSafeKey> {
    UnboundKey::new(&AES_256_GCM, key)
        .map(LessSafeKey::new)
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a credential sealed here opens back to exactly what was sealed, under the key
    /// that sealed it, and nothing about the sealing depends on the plaintext.
    ///
    /// The rows span what the pass generalises over: the plaintext's length (one byte, the
    /// width of a minted credential, a long one), its content (text, bytes that are not
    /// text, bytes that are all zero) and the key it was sealed under (each row seals under
    /// its own).
    #[test]
    fn a_sealed_credential_opens_back_as_what_was_sealed() {
        let long = vec![b'r'; 512];
        let cases: [(&str, &[u8]); 5] = [
            ("one byte", b"r"),
            ("a minted credential", b"6RkHPrWRMvbVkqQmGE6iQ3zEcy0mYi1n"),
            ("bytes that are not text", &[0, 159, 146, 150, 255]),
            ("bytes that are all zero", &[0; 32]),
            ("a plaintext longer than the modulus", &long),
        ];
        for (case, plaintext) in cases {
            let key = RefreshTokenKey::generate();
            let sealed = key
                .seal(plaintext)
                .unwrap_or_else(|| panic!("case {case:?}: the credential seals"));
            let opened = key.open(&sealed);
            assert_eq!(
                opened.as_deref(),
                Some(plaintext),
                "case {case:?}: expected the sealed plaintext back, got {opened:?}"
            );
        }
    }

    /// RULE: every value a seal produces is a JWE in compact serialization — exactly
    /// [`COMPACT_SEGMENTS`] non-empty base64url segments, the first of which decodes to the
    /// JOSE header naming the two algorithms — and carries no run of the plaintext.
    #[test]
    fn a_sealed_credential_is_a_compact_jwe_naming_its_algorithms() {
        for case in ["a credential", "another credential", "a third one"] {
            let key = RefreshTokenKey::generate();
            let sealed = key
                .seal(case.as_bytes())
                .unwrap_or_else(|| panic!("case {case:?}: the credential seals"));
            let segments: Vec<&str> = sealed.split('.').collect();
            assert_eq!(
                segments.len(),
                COMPACT_SEGMENTS,
                "case {case:?}: expected {COMPACT_SEGMENTS} segments, got {segments:?}"
            );
            for (at, segment) in segments.iter().enumerate() {
                assert!(
                    !segment.is_empty()
                        && segment
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')),
                    "case {case:?}: segment {at} is base64url and non-empty, got {segment:?}"
                );
            }
            let header: serde_json::Value = base64_url_decode(segments[0])
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the first segment decodes to a JOSE header")
                });
            assert_eq!(
                header["alg"], KEY_MANAGEMENT_ALGORITHM,
                "case {case:?}: expected alg {KEY_MANAGEMENT_ALGORITHM}, got {header}"
            );
            assert_eq!(
                header["enc"], CONTENT_ENCRYPTION_ALGORITHM,
                "case {case:?}: expected enc {CONTENT_ENCRYPTION_ALGORITHM}, got {header}"
            );
            assert!(
                !sealed.contains(case),
                "case {case:?}: the plaintext is sealed, found it in {sealed}"
            );
        }
    }

    /// RULE: a value this key did not seal opens to nothing — whichever part of the token
    /// differs, and whoever holds another key.
    #[test]
    fn a_token_this_key_did_not_seal_opens_to_nothing() {
        let key = RefreshTokenKey::generate();
        let sealed = key.seal(b"a credential").expect("the credential seals");
        let segments: Vec<&str> = sealed.split('.').collect();

        // (case, the token presented)
        let altered_header = format!(
            "{}.{}",
            base64_url(br#"{"alg":"dir","enc":"A256GCM"}"#),
            segments[1..].join(".")
        );
        let truncated = segments[..COMPACT_SEGMENTS - 1].join(".");
        let flipped_ciphertext = {
            let mut bytes = base64_url_decode(segments[3]).expect("a base64url segment");
            bytes[0] ^= 0x01;
            format!(
                "{}.{}.{}.{}.{}",
                segments[0],
                segments[1],
                segments[2],
                base64_url(&bytes),
                segments[4]
            )
        };
        let another_key = RefreshTokenKey::generate();
        let cases: [(&str, String, &RefreshTokenKey); 5] = [
            ("a value that is not a JOSE token at all", "not-a-token".to_string(), &key),
            ("a token one segment short", truncated, &key),
            ("a header naming another algorithm", altered_header, &key),
            ("a ciphertext altered after it was sealed", flipped_ciphertext, &key),
            ("the token opened under another key", sealed.clone(), &another_key),
        ];
        for (case, presented, opening) in cases {
            let opened = opening.open(&presented);
            assert_eq!(
                opened, None,
                "case {case:?}: expected no plaintext, got {opened:?}"
            );
        }
    }
}
