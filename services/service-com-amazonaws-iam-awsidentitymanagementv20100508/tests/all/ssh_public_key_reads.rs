// SPDX-License-Identifier: CC0-1.0

//! Reading ONE of the SSH public keys an IAM user holds, end to end over the awsQuery wire:
//! `GetSSHPublicKey` against what `UploadSSHPublicKey` stored.
//!
//! The rules under test:
//!
//! 1. the read reports the STORED key under the id the upload minted for it, with its body
//!    written in the encoding the request asks for — "specifies the public key encoding
//!    format to use in the response. To retrieve the public key in ssh-rsa format, use SSH.
//!    To retrieve the public key in PEM format, use PEM" (`GetSSHPublicKey$Encoding`) —
//!    while every other member (`SSHPublicKeyId`, `UserName`, `Fingerprint`, `Status`,
//!    `UploadDate`) is what the account holds, independent of the encoding asked for,
//!    because `Fingerprint` is "the MD5 message digest of the SSH public key" and a digest
//!    is of the key rather than of the text that carried it.
//! 2. a key is reachable only through the user it is associated with, so a user the account
//!    does not hold, an id it never minted and an id another user holds are each
//!    `NoSuchEntity` (404); and the read writes nothing, so what the next caller sees is
//!    unchanged by it.
//!
//! Each suite is a TABLE whose rows differ along what rule 1 generalises over: which
//! encoding carried the key in, which encoding the response is asked for, how long the key
//! is, and whether the line that carried it had a comment. The expected key is read back
//! out of the response by the crates that implement these formats — never by comparing the
//! response with the request text — so a row asserts the KEY round-tripped, not that one
//! blob was echoed.
//!
//! The material fixtures (`modulus`, `openssh_line`, `spki_pem`, `pkcs1_pem`), the upload
//! and the user fixture are the ones `tests/all/ssh_public_keys.rs` owns; the awsQuery
//! harness is `tests/all/user_lifecycle.rs`'s and the signed-caller fixture
//! `tests/all/identity_policy_enforcement.rs`'s.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::ENCODING_TYPE_VALUES;

use crate::identity_policy_enforcement::{principal, statement};
use crate::ssh_public_keys::{
    assert_fingerprint, create_user, modulus, openssh_line, pkcs1_pem, spki_pem, upload,
    PUBLISHED_MINIMUM_BITS,
};
use crate::user_lifecycle::{call, call_signed_by, element, elements, error_code};

/// The public exponent every key below carries: 65537, the exponent `ssh-keygen` writes —
/// the same one the material fixtures build their keys with.
const EXPONENT: [u8; 3] = [0x01, 0x00, 0x01];

/// The two values `encodingType` declares, written out here rather than read from the
/// implementation (`smithy_query(query="shape", shape="encodingType")` gives `SSH`, `PEM`).
const PUBLISHED_ENCODINGS: [&str; 2] = ["SSH", "PEM"];

/// RULE 1: the encodings a response can be asked for are the ones the model declares.
#[test]
fn the_published_encodings_are_the_ones_the_model_declares() {
    let mut modelled: Vec<&str> = ENCODING_TYPE_VALUES.to_vec();
    modelled.sort_unstable();
    let mut published: Vec<&str> = PUBLISHED_ENCODINGS.to_vec();
    published.sort_unstable();
    assert_eq!(
        modelled, published,
        "expected encodingType to declare {published:?}, got {modelled:?}"
    );
}

/// One row of the stored-key table: the body an upload carries, and the key it carries.
struct Stored {
    case: &'static str,
    body: String,
    modulus: Vec<u8>,
}

/// One row, from the key it is built on and the way that key is written out.
fn stored_row(case: &'static str, material: Vec<u8>, write: impl Fn(&[u8]) -> String) -> Stored {
    Stored {
        case,
        body: write(&material),
        modulus: material,
    }
}

/// Rows spanning both encodings a key can arrive in, both PEM labels a public key is
/// written under, the published minimum key length and twice it, and a line with a comment
/// on it and one without.
fn stored() -> Vec<Stored> {
    vec![
        stored_row(
            "an ssh-rsa line at the published minimum length",
            modulus(PUBLISHED_MINIMUM_BITS, 101),
            |material| openssh_line(material, ""),
        ),
        stored_row(
            "an ssh-rsa line carrying a comment",
            modulus(PUBLISHED_MINIMUM_BITS, 102),
            |material| openssh_line(material, "developer@workstation"),
        ),
        stored_row(
            "an ssh-rsa line twice the minimum length",
            modulus(PUBLISHED_MINIMUM_BITS * 2, 103),
            |material| openssh_line(material, ""),
        ),
        stored_row(
            "a SubjectPublicKeyInfo PEM",
            modulus(PUBLISHED_MINIMUM_BITS, 104),
            spki_pem,
        ),
        stored_row(
            "a PKCS#1 RSA public key PEM",
            modulus(PUBLISHED_MINIMUM_BITS, 105),
            pkcs1_pem,
        ),
        stored_row(
            "a SubjectPublicKeyInfo PEM twice the minimum length",
            modulus(PUBLISHED_MINIMUM_BITS * 2, 106),
            spki_pem,
        ),
    ]
}

/// RULE 1 over every pairing of the encoding a key arrived in with the encoding it is read
/// back in: the body carries THAT key written that way, and every other member is the one
/// the account holds.
#[tokio::test]
async fn a_stored_key_is_reported_in_each_encoding_asked_for() {
    let router = crate::user_lifecycle::router();

    for (row, stored) in stored().into_iter().enumerate() {
        let user_name = format!("keyreader{row}");
        create_user(&router, stored.case, &user_name).await;

        let (status, uploaded) = upload(&router, &user_name, &stored.body).await;
        assert_eq!(
            status, 200,
            "case {:?}: the fixture upload must be served, got {status}: {uploaded}",
            stored.case
        );
        let ssh_public_key_id = element(&uploaded, "SSHPublicKeyId");
        let fingerprint = element(&uploaded, "Fingerprint");
        let upload_date = element(&uploaded, "UploadDate");

        for encoding in PUBLISHED_ENCODINGS {
            let case = format!("{} read back as {encoding}", stored.case);
            let (status, read) = get(&router, &user_name, &ssh_public_key_id, encoding).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the read to be served, got {status}: {read}"
            );
            assert_eq!(
                element(&read, "SSHPublicKeyId"),
                ssh_public_key_id,
                "case {case:?}: expected the stored key's id, got {read}"
            );
            assert_eq!(
                element(&read, "UserName"),
                user_name,
                "case {case:?}: expected the key to name the user holding it, got {read}"
            );
            assert_eq!(
                element(&read, "Status"),
                "Active",
                "case {case:?}: expected the stored status, got {read}"
            );
            assert_eq!(
                element(&read, "UploadDate"),
                upload_date,
                "case {case:?}: expected the stored upload date, got {read}"
            );
            // The digest is of the KEY, so asking for the other encoding cannot move it.
            assert_eq!(
                element(&read, "Fingerprint"),
                fingerprint,
                "case {case:?}: expected the stored fingerprint, got {read}"
            );
            assert_fingerprint(&case, &element(&read, "Fingerprint"));

            let body = element(&read, "SSHPublicKeyBody");
            let Some(carried) = read_public_key(&body) else {
                panic!("case {case:?}: expected a readable public key, got {body:?}");
            };
            assert_eq!(
                (carried.modulus, carried.public_exponent),
                (stored.modulus.clone(), EXPONENT.to_vec()),
                "case {case:?}: expected the body to carry the stored key, got {body:?}"
            );
            assert_eq!(
                carried.encoding, encoding,
                "case {case:?}: expected the body to be written in the encoding asked \
                 for, got {body:?}"
            );
        }
    }
}

/// RULE 1: the read reports a stored key and writes nothing, so reading it — in either
/// encoding, any number of times — leaves what the next caller sees untouched.
#[tokio::test]
async fn reading_a_key_leaves_it_as_the_account_holds_it() {
    let router = crate::user_lifecycle::router();
    create_user(&router, "repeated reads", "rereader").await;
    let body = openssh_line(&modulus(PUBLISHED_MINIMUM_BITS, 111), "");
    let (status, uploaded) = upload(&router, "rereader", &body).await;
    assert_eq!(
        status, 200,
        "the fixture upload must be served, got {status}: {uploaded}"
    );
    let ssh_public_key_id = element(&uploaded, "SSHPublicKeyId");

    let mut answers: Vec<String> = Vec::new();
    for round in 0..2 {
        for encoding in PUBLISHED_ENCODINGS {
            let case = format!("round {round} as {encoding}");
            let (status, read) = get(&router, "rereader", &ssh_public_key_id, encoding).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the read to be served, got {status}: {read}"
            );
            answers.push(element(&read, "SSHPublicKeyBody"));
        }
    }
    assert_eq!(
        answers[0], answers[2],
        "expected a repeated SSH read to answer alike, got {answers:?}"
    );
    assert_eq!(
        answers[1], answers[3],
        "expected a repeated PEM read to answer alike, got {answers:?}"
    );

    let (status, listed) = call(
        &router,
        "ListSSHPublicKeys",
        &[("UserName", "rereader")],
    )
    .await;
    assert_eq!(status, 200, "the list must be served, got {status}: {listed}");
    assert_eq!(
        elements(&listed, "SSHPublicKeyId"),
        vec![ssh_public_key_id],
        "expected the read to leave exactly the stored key, got {listed}"
    );
    assert_eq!(
        elements(&listed, "Status"),
        vec!["Active".to_string()],
        "expected the read to leave the stored status, got {listed}"
    );
}

/// RULE 2 over every way a request can name no key the account holds: each is
/// `NoSuchEntity` (404), worded for the entity that is missing.
#[tokio::test]
async fn a_key_the_named_user_does_not_hold_is_refused() {
    let router = crate::user_lifecycle::router();
    create_user(&router, "absent keys", "holder").await;
    create_user(&router, "absent keys", "bystander").await;
    let (status, uploaded) = upload(
        &router,
        "holder",
        &openssh_line(&modulus(PUBLISHED_MINIMUM_BITS, 121), ""),
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture upload must be served, got {status}: {uploaded}"
    );
    let held = element(&uploaded, "SSHPublicKeyId");

    // An id satisfying `publicKeyIdType` (len 20..128, pattern `^[\w]+$`) that the account
    // never minted.
    let unminted = "APKANEVERMINTEDKEY00";

    // (case, user named, id named, the message the refusal carries)
    let rows: Vec<(&str, &str, &str, String)> = vec![
        (
            "a user the account holds no record of",
            "no_such_user",
            &held,
            "The user with name no_such_user cannot be found.".to_string(),
        ),
        (
            "an id the account never minted",
            "holder",
            unminted,
            format!("The SSH Public Key with id {unminted} cannot be found."),
        ),
        (
            "an id another user holds",
            "bystander",
            &held,
            format!("The SSH Public Key with id {held} cannot be found."),
        ),
    ];

    for (case, user_name, ssh_public_key_id, message) in rows {
        for encoding in PUBLISHED_ENCODINGS {
            let (status, refused) = get(&router, user_name, ssh_public_key_id, encoding).await;
            assert_eq!(
                (status, error_code(&refused)),
                (404, "NoSuchEntity".to_string()),
                "case {case:?} as {encoding}: expected the read to be refused, got \
                 {status}: {refused}"
            );
            assert_eq!(
                element(&refused, "Message"),
                message,
                "case {case:?} as {encoding}: expected the refusal to name the missing \
                 entity, got {refused}"
            );
        }
    }

    // The key is still there for the user that holds it: a refusal reported nothing about
    // it and removed nothing.
    let (status, read) = get(&router, "holder", &held, "SSH").await;
    assert_eq!(
        status, 200,
        "the held key must still be served, got {status}: {read}"
    );
}

/// RULE 1's value set: an `Encoding` outside the two values `encodingType` declares is
/// refused as the modelled constraint it violates, and no key is reported.
#[tokio::test]
async fn an_encoding_outside_the_modelled_set_is_refused() {
    let router = crate::user_lifecycle::router();
    create_user(&router, "encodings", "encoded").await;
    let (status, uploaded) = upload(
        &router,
        "encoded",
        &openssh_line(&modulus(PUBLISHED_MINIMUM_BITS, 131), ""),
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture upload must be served, got {status}: {uploaded}"
    );
    let held = element(&uploaded, "SSHPublicKeyId");

    // Rows: another encoding name entirely, each modelled value in the wrong case, and an
    // empty value.
    for encoding in ["DER", "OPENSSH", "ssh", "pem", ""] {
        let (status, refused) = get(&router, "encoded", &held, encoding).await;
        assert_eq!(
            (status, error_code(&refused)),
            (400, "ValidationError".to_string()),
            "case {encoding:?}: expected an encoding outside the modelled set to be \
             refused, got {status}: {refused}"
        );
        assert!(
            elements(&refused, "SSHPublicKeyBody").is_empty(),
            "case {encoding:?}: a refused read must report no key, got {refused}"
        );
    }

    // And each modelled value, spelled as the enum declares it, is served.
    for encoding in ENCODING_TYPE_VALUES {
        let (status, read) = get(&router, "encoded", &held, encoding).await;
        assert_eq!(
            status, 200,
            "case {encoding:?}: a modelled encoding must be served, got {status}: {read}"
        );
    }
}

/// RULE: IAM authorizes its own API, so the read is decided against the signing principal's
/// identity-based policies before any key is reported.
#[tokio::test]
async fn the_read_is_decided_against_the_callers_policies() {
    let router = crate::user_lifecycle::router();

    // (the action granted, whether the read is then served)
    let rows: &[(&str, bool)] = &[("iam:GetSSHPublicKey", true), ("iam:GetUser", false)];

    for (row, (granted, served)) in rows.iter().enumerate() {
        let user_name = format!("gatedreader{row}");
        let access_key_id = principal(
            &router,
            granted,
            &user_name,
            &[("grant".to_string(), statement("Allow", granted, "*"))],
        )
        .await;
        let (status, uploaded) = upload(
            &router,
            &user_name,
            &openssh_line(&modulus(PUBLISHED_MINIMUM_BITS, 141 + row as u8), ""),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {granted:?}: the fixture upload must be served, got {status}: {uploaded}"
        );
        let held = element(&uploaded, "SSHPublicKeyId");

        let (status, answered) = call_signed_by(
            &router,
            Some(&access_key_id),
            "GetSSHPublicKey",
            &[
                ("UserName", &user_name),
                ("SSHPublicKeyId", &held),
                ("Encoding", "SSH"),
            ],
        )
        .await;
        if *served {
            assert_eq!(
                status, 200,
                "case {granted:?}: expected the grant to serve the read, got {status}: \
                 {answered}"
            );
        } else {
            assert_eq!(
                (status, error_code(&answered)),
                (403, "AccessDenied".to_string()),
                "case {granted:?}: expected the read to be refused, got {status}: {answered}"
            );
        }
    }
}

/// One `GetSSHPublicKey` call.
pub(crate) async fn get(
    router: &MultiServiceRouter,
    user_name: &str,
    ssh_public_key_id: &str,
    encoding: &str,
) -> (u16, String) {
    call(
        router,
        "GetSSHPublicKey",
        &[
            ("UserName", user_name),
            ("SSHPublicKeyId", ssh_public_key_id),
            ("Encoding", encoding),
        ],
    )
    .await
}

/// The public key one response body carries: the encoding it is written in, and the RSA key
/// itself as the big-endian integers that identify it.
struct CarriedKey {
    encoding: &'static str,
    modulus: Vec<u8>,
    public_exponent: Vec<u8>,
}

/// Read `body` as the public key it carries, or `None` when it carries none.
///
/// Read by the crates that implement these formats, independently of how the service
/// writes them, so a row compares KEYS rather than text.
fn read_public_key(body: &str) -> Option<CarriedKey> {
    use ::pkcs1::der::Decode as _;

    let text = body.trim();
    if let Ok(key) = ::ssh_key::PublicKey::from_openssh(text) {
        let ::ssh_key::public::KeyData::Rsa(rsa) = key.key_data() else {
            return None;
        };
        return Some(CarriedKey {
            encoding: "SSH",
            modulus: rsa.n.as_positive_bytes()?.to_vec(),
            public_exponent: rsa.e.as_positive_bytes()?.to_vec(),
        });
    }
    let (label, document) = ::pkcs1::der::Document::from_pem(text).ok()?;
    let der = document.as_bytes();
    let key = match label {
        "RSA PUBLIC KEY" => ::pkcs1::RsaPublicKey::from_der(der).ok()?,
        "PUBLIC KEY" => {
            let info = ::pkcs8::spki::SubjectPublicKeyInfoRef::from_der(der).ok()?;
            ::pkcs1::RsaPublicKey::from_der(info.subject_public_key.as_bytes()?).ok()?
        }
        _ => return None,
    };
    Some(CarriedKey {
        encoding: "PEM",
        modulus: key.modulus.as_bytes().to_vec(),
        public_exponent: key.public_exponent.as_bytes().to_vec(),
    })
}
