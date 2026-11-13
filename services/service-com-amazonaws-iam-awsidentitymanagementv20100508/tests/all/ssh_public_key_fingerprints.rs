// SPDX-License-Identifier: CC0-1.0

//! What `UploadSSHPublicKey` answers ABOUT the key it took: the fingerprint it computes for
//! it, and the sentence it refuses a key the named user already holds with.
//!
//! The rules under test:
//!
//! 1. `Fingerprint` is "the MD5 message digest of the SSH public key"
//!    (`smithy_query(query="doc", shape="com.amazonaws.iam#SSHPublicKey",
//!    member="Fingerprint")`) — the digest of the KEY as the SSH public key format writes
//!    it, which is the blob an `ssh-rsa` line carries base64-encoded, rendered as the
//!    colon-separated lowercase hex pairs `publicKeyFingerprintType` (`^[:\w]+$`) admits.
//!    That is the value `ssh-keygen -l -E md5` prints for the same key: a function of the
//!    key alone, so the encoding that carried the key in does not enter it and every later
//!    read reports the same string.
//! 2. an upload of a key the named user already holds is refused with
//!    `DuplicateSSHPublicKey` (400) and the one sentence AWS words that refusal with, which
//!    names neither the key nor the user because the request already named both.
//!
//! Each suite is a TABLE whose rows differ along what the rules generalise over: how long
//! the key is, which encoding carried it in, and whether the line that carried it had a
//! comment. Rule 1's expected digest is NOT read from the implementation: every row writes
//! its own key as an `ssh-rsa` line with the suite's fixture, takes the blob off that line
//! and digests it, exactly as a client verifying a fingerprint does.
//!
//! The material fixtures (`modulus`, `openssh_line`, `spki_pem`, `pkcs1_pem`), the upload
//! and the user fixture are the ones `tests/all/ssh_public_keys.rs` owns; the awsQuery
//! harness is `tests/all/user_lifecycle.rs`'s.

use ::md5::Digest as _;

// The read this suite checks the stored key through is `tests/all/ssh_public_key_reads.rs`'s,
// which owns the rest of what a read reports.
use crate::ssh_public_key_reads::get;
use crate::ssh_public_keys::{
    assert_fingerprint, create_user, modulus, openssh_line, pkcs1_pem, spki_pem, upload,
    PUBLISHED_MINIMUM_BITS,
};
use crate::user_lifecycle::{element, error_code, router};

/// The sentence AWS answers an upload of a key the user already holds with — the whole
/// `Message` of `DuplicateSSHPublicKeyException`, fixed text with no variable part, since
/// IAM names neither the key nor the id it is held under.
const DUPLICATE_MESSAGE: &str = "Duplicate SSH public key uploaded.";

/// One row of the fingerprinted-key table: the body an upload carries, what it carries it
/// as, and the key itself, which the row's expected digest is taken over.
struct Fingerprinted {
    case: &'static str,
    body: String,
    modulus: Vec<u8>,
}

/// One row, from the key it is built on and the way that key is written out.
fn fingerprinted_row(
    case: &'static str,
    material: Vec<u8>,
    write: impl Fn(&[u8]) -> String,
) -> Fingerprinted {
    Fingerprinted {
        case,
        body: write(&material),
        modulus: material,
    }
}

/// The rows: the shortest key AWS takes and two longer ones, in each encoding a body may
/// arrive in, with and without a comment on the line.
fn fingerprinted() -> Vec<Fingerprinted> {
    vec![
        fingerprinted_row(
            "an ssh-rsa line carrying the shortest key AWS takes",
            modulus(PUBLISHED_MINIMUM_BITS, 41),
            |material| openssh_line(material, ""),
        ),
        fingerprinted_row(
            "an ssh-rsa line with a comment on it",
            modulus(PUBLISHED_MINIMUM_BITS, 42),
            |material| openssh_line(material, "a comment names no key"),
        ),
        fingerprinted_row(
            "an ssh-rsa line carrying a 3072-bit key",
            modulus(3072, 43),
            |material| openssh_line(material, ""),
        ),
        fingerprinted_row(
            "a SubjectPublicKeyInfo PEM",
            modulus(PUBLISHED_MINIMUM_BITS, 44),
            spki_pem,
        ),
        fingerprinted_row(
            "a PKCS#1 PEM carrying a 4096-bit key",
            modulus(4096, 45),
            pkcs1_pem,
        ),
    ]
}

/// RULE 1: the fingerprint an upload reports is the MD5 digest of the SSH public key, and
/// the stored key reports that same digest when it is read back.
#[tokio::test]
async fn an_uploaded_key_is_fingerprinted_as_the_digest_of_the_ssh_public_key() {
    let router = router();

    for (row, case) in fingerprinted().into_iter().enumerate() {
        let user_name = format!("fingerprintholder{row}");
        create_user(&router, case.case, &user_name).await;

        let (status, uploaded) = upload(&router, &user_name, &case.body).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the upload to be served, got {status}: {uploaded}",
            case.case
        );
        let fingerprint = element(&uploaded, "Fingerprint");
        assert_fingerprint(case.case, &fingerprint);

        // The expected digest, taken over the row's OWN key: the suite writes that key as
        // an `ssh-rsa` line with its fixture and digests the blob that line carries, so
        // what the row asserts comes from the key it uploaded rather than from anything the
        // service answered.
        let expected = digest_of_key_blob(case.case, &openssh_line(&case.modulus, ""));
        assert_eq!(
            fingerprint, expected,
            "case {:?}: expected the MD5 digest of the SSH public key {expected:?}, got \
             {fingerprint:?}",
            case.case
        );

        // And the stored key reports that one digest, beside the `ssh-rsa` line it writes
        // out for the same key, whichever encoding carried the key in.
        let ssh_public_key_id = element(&uploaded, "SSHPublicKeyId");
        let (status, read) = get(&router, &user_name, &ssh_public_key_id, "SSH").await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the read to be served, got {status}: {read}",
            case.case
        );
        assert_eq!(
            digest_of_key_blob(case.case, &element(&read, "SSHPublicKeyBody")),
            expected,
            "case {:?}: expected the read to report the same key, got {read}",
            case.case
        );
        assert_eq!(
            element(&read, "Fingerprint"),
            fingerprint,
            "case {:?}: expected the stored key to report one fingerprint, got {read}",
            case.case
        );
    }
}

// RULE 1's other invariant — one key fingerprints alike in every encoding, and two
// different keys fingerprint differently — is `tests/all/ssh_public_keys.rs`'s
// (`one_key_fingerprints_alike_in_every_encoding`), so it is not repeated here; what this
// suite adds is WHICH digest that one value is.

/// RULE 2: a key the named user already holds is refused with the sentence AWS words that
/// refusal with, whichever encoding the repeat arrives in and however long the key is.
#[tokio::test]
async fn a_repeat_upload_is_refused_with_the_duplicate_sentence() {
    let router = router();

    // The rows: a key of each length, first uploaded as one encoding and then repeated in
    // another, so no row depends on the text of the body that carried it.
    for (row, (case, bits, seed)) in [
        ("the shortest key AWS takes", PUBLISHED_MINIMUM_BITS, 51u8),
        ("a 3072-bit key", 3072, 52),
        ("a 4096-bit key", 4096, 53),
    ]
    .into_iter()
    .enumerate()
    {
        let user_name = format!("repeatholder{row}");
        create_user(&router, case, &user_name).await;
        let material = modulus(bits, seed);
        let first = openssh_line(&material, "");
        let (status, uploaded) = upload(&router, &user_name, &first).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the first upload to be served, got {status}: {uploaded}"
        );

        for (repeat, body) in [
            ("the same line again", first.clone()),
            (
                "the same key with a comment on it",
                openssh_line(&material, "a repeat under another name"),
            ),
            ("the same key as a PEM", spki_pem(&material)),
            ("the same key as a PKCS#1 PEM", pkcs1_pem(&material)),
        ] {
            let (status, refused) = upload(&router, &user_name, &body).await;
            assert_eq!(
                (status, error_code(&refused)),
                (400, "DuplicateSSHPublicKey".to_string()),
                "case {case:?} repeated as {repeat:?}: expected the repeat to be refused, got \
                 {status}: {refused}"
            );
            assert_eq!(
                element(&refused, "Message"),
                DUPLICATE_MESSAGE,
                "case {case:?} repeated as {repeat:?}: expected the duplicate sentence, got \
                 {refused}"
            );
        }
    }
}

/// The MD5 digest of the SSH public key the `ssh-rsa` line `line` carries, in the
/// colon-separated lowercase hex pairs a hex fingerprint is published in.
///
/// The blob is taken off the LINE — its base64 field, decoded by the crate this suite
/// spells base64 with — so what is digested here is the key as the wire carries it,
/// independently of how the service arrived at its own answer.
fn digest_of_key_blob(case: &str, line: &str) -> String {
    let mut fields = line.split_whitespace();
    assert_eq!(
        fields.next(),
        Some("ssh-rsa"),
        "case {case:?}: expected an ssh-rsa line, got {line:?}"
    );
    let encoded = fields
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: the ssh-rsa line carries no blob: {line:?}"));
    let blob = ::data_encoding::BASE64
        .decode(encoded.as_bytes())
        .unwrap_or_else(|error| {
            panic!("case {case:?}: the ssh-rsa blob must be base64: {error} in {line:?}")
        });
    let digest = ::md5::Md5::digest(&blob);
    let hex = ::data_encoding::HEXLOWER.encode(&digest);
    let pairs: Vec<String> = hex
        .as_bytes()
        .chunks(2)
        .map(|pair| String::from_utf8_lossy(pair).into_owned())
        .collect();
    pairs.join(":")
}

