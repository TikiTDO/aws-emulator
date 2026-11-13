// SPDX-License-Identifier: CC0-1.0

//! The SSH public keys an IAM user holds: what counts as one, how the account identifies
//! and fingerprints it, and the record every operation on one reads.
//!
//! "Uploads an SSH public key and associates it with the specified IAM user … used only for
//! authenticating the associated IAM user to an CodeCommit repository"
//! (`UploadSSHPublicKey`), and "returns information about the SSH public keys associated
//! with the specified IAM user. If none exists, the operation returns an empty list"
//! (`ListSSHPublicKeys`) — so a user's keys are a collection of the ACCOUNT's, written by
//! the upload and reported by the list, and a user that holds none is reported as holding
//! none.
//!
//! The material is read by the crates that implement the formats it comes in — [`ssh_key`]
//! for the OpenSSH public key line, [`pkcs1`] and `spki` for the PEM structures — rather
//! than by matching on the text: what counts as a key is the format itself, which is also
//! what tells the two refusals apart. An encoding neither family recognises is
//! `UnrecognizedPublicKeyEncodingException`; material in a recognised encoding that is not
//! a usable key is `InvalidPublicKeyException`.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every
//! other primitive there.

use ::md5::Digest as _;
use ::pkcs1::der::{Decode as _, Encode as _};

use super::{
    name_key, unique_id, ComAmazonawsIamAwsidentitymanagementv20100508State as IamState,
    CREDENTIAL_STATUS_ACTIVE,
};
use crate::ops::types::{SSHPublicKey, SSHPublicKeyMetadata};

/// The prefix AWS gives an SSH public key's unique id — the `APKA` prefix the "IAM
/// identifiers" page of the IAM User Guide lists for a public key under "unique ID
/// prefixes". `SSHPublicKey$SSHPublicKeyId` is modelled as `publicKeyIdType` (len 20..128,
/// pattern `^[\w]+$`), which this prefix plus the account's unique-id suffix satisfies.
const SSH_PUBLIC_KEY_ID_PREFIX: &str = "APKA";

/// How many SSH public keys ONE IAM user may hold. AWS's "IAM and STS quotas" page gives
/// the quota "SSH public keys per IAM user: 5"; an upload past it is refused with
/// `LimitExceededException`, whose documentation says "the error message describes the
/// limit exceeded", and stores nothing.
pub const MAX_SSH_PUBLIC_KEYS_PER_USER: usize = 5;

/// The shortest RSA key an upload takes, in bits: "the minimum bit-length of the public key
/// is 2048 bits" (`UploadSSHPublicKey$SSHPublicKeyBody`).
pub const MIN_SSH_PUBLIC_KEY_BITS: usize = 2048;

/// The key type an OpenSSH public key line carries for the format the upload names:
/// "the public key must be encoded in ssh-rsa format or PEM format".
const OPENSSH_RSA_TYPE: &str = "ssh-rsa";

/// The opening of the PEM framing RFC 7468 defines, which is what tells a PEM body from an
/// OpenSSH line without reading either as the other.
const PEM_BEGIN: &str = "-----BEGIN ";

/// The PEM label of an X.509 `SubjectPublicKeyInfo` (RFC 7468 section 13) — what
/// `ssh-keygen -e -m PKCS8` writes, the PEM form of a public key.
const PEM_LABEL_SUBJECT_PUBLIC_KEY_INFO: &str = "PUBLIC KEY";

/// The PEM label of a PKCS#1 `RSAPublicKey` (RFC 7468 section 7) — what
/// `ssh-keygen -e -m PEM` writes for an RSA key.
const PEM_LABEL_RSA_PUBLIC_KEY: &str = "RSA PUBLIC KEY";

/// `encodingType`'s `SSH` (`smithy_query(query="shape", shape="encodingType")` gives
/// `SSH`, `PEM`): "to retrieve the public key in ssh-rsa format, use SSH"
/// (`GetSSHPublicKey$Encoding`).
pub const ENCODING_SSH: &str = "SSH";

/// `encodingType`'s `PEM`: "to retrieve the public key in PEM format, use PEM"
/// (`GetSSHPublicKey$Encoding`).
pub const ENCODING_PEM: &str = "PEM";

/// One SSH public key an IAM user holds, keyed in
/// [`IamState::ssh_public_keys`] by its `SSHPublicKeyId` — the handle every other
/// operation on one takes (`GetSSHPublicKey`, `UpdateSSHPublicKey` and
/// `DeleteSSHPublicKey` name the key by id, and `ListSSHPublicKeys` reports it).
///
/// The owning user's NAME is not copied in: it is read from the user the record's
/// [`user_key`](SshPublicKeyRecord::user_key) names, so the name lives in exactly one place
/// and a user renamed by an update does not leave a stale copy here.
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct SshPublicKeyRecord {
    /// [`name_key`] of the IAM user that holds the key.
    pub user_key: String,
    /// "The SSH public key" (`SSHPublicKey$SSHPublicKeyBody`), verbatim as the upload
    /// carried it, so a later read reports the body the caller sent byte for byte.
    pub ssh_public_key_body: String,
    /// The DER `SubjectPublicKeyInfo` the body decodes to: what tells one key from
    /// another, independently of the encoding that carried it and of any comment on the
    /// line.
    ///
    /// Kept beside the body so recognising a repeat upload costs one comparison per held
    /// key instead of re-reading every held body. It is DERIVED from
    /// [`ssh_public_key_body`](Self::ssh_public_key_body) and reported by nothing: no
    /// modelled member carries it.
    pub subject_public_key_info: Vec<u8>,
    /// "The MD5 message digest of the SSH public key" (`SSHPublicKey$Fingerprint`), as
    /// [`read_ssh_public_key`] derives it from the key material and from nothing else.
    pub fingerprint: String,
    /// The key's `statusType` value: [`CREDENTIAL_STATUS_ACTIVE`] when it was uploaded,
    /// until an update moves it — "`Active` means that the key can be used for
    /// authentication with an CodeCommit repository" (`SSHPublicKey$Status`).
    pub status: String,
    /// "The date and time … when the SSH public key was uploaded"
    /// (`SSHPublicKey$UploadDate`), reported unchanged by every later read.
    #[serde(with = "::service_common::persist::smithy_time")]
    pub upload_date: ::aws_smithy_types::DateTime,
}

impl SshPublicKeyRecord {
    /// This record as the whole modelled shape, under the id it is keyed by and the user
    /// name as the account holds it.
    ///
    /// ONE place builds the shape, so the upload response and every later read report the
    /// same key identically.
    pub fn report(&self, ssh_public_key_id: &str, user_name: &str) -> SSHPublicKey {
        SSHPublicKey {
            user_name: user_name.to_string(),
            ssh_public_key_id: ssh_public_key_id.to_string(),
            fingerprint: self.fingerprint.clone(),
            ssh_public_key_body: self.ssh_public_key_body.clone(),
            status: self.status.clone(),
            upload_date: Some(self.upload_date),
        }
    }

    /// This record as the whole modelled shape, with its body written in the encoding
    /// `encoding` names, or `None` for an encoding outside the value set `encodingType`
    /// declares.
    ///
    /// "Specifies the public key encoding format to use in the response. To retrieve the
    /// public key in ssh-rsa format, use SSH. To retrieve the public key in PEM format, use
    /// PEM" (`GetSSHPublicKey$Encoding`) — so the ENCODING is the caller's choice while the
    /// KEY is the account's: every other member is reported exactly as
    /// [`report`](Self::report) reports it, including the fingerprint, which is a digest of
    /// the key material and therefore the same whichever encoding carries it.
    pub fn report_in(
        &self,
        ssh_public_key_id: &str,
        user_name: &str,
        encoding: &str,
    ) -> Option<SSHPublicKey> {
        Some(SSHPublicKey {
            ssh_public_key_body: self.body_in(encoding)?,
            ..self.report(ssh_public_key_id, user_name)
        })
    }

    /// The key's body written in the encoding `encoding` names, or `None` for an encoding
    /// outside the value set `encodingType` declares.
    ///
    /// A body already written in the encoding asked for is reported verbatim, so the key
    /// the upload carried comes back byte for byte in its own encoding. In the OTHER
    /// encoding the body is written out from the KEY the account holds — the DER
    /// `SubjectPublicKeyInfo` every accepted body was reduced to — by the crates that
    /// implement these formats, so one key renders alike however it arrived.
    pub fn body_in(&self, encoding: &str) -> Option<String> {
        if body_encoding(&self.ssh_public_key_body) == Some(encoding) {
            return Some(self.ssh_public_key_body.clone());
        }
        match encoding {
            ENCODING_SSH => openssh_public_key_line(&self.subject_public_key_info),
            ENCODING_PEM => subject_public_key_info_pem(&self.subject_public_key_info),
            _ => None,
        }
    }

    /// This record as the shape a LIST reports: "contains information about an SSH public
    /// key, without the key's body or fingerprint" (`SSHPublicKeyMetadata`) — the same
    /// four members, read from the same record, so a listing and a read cannot disagree
    /// about one key.
    pub fn report_metadata(
        &self,
        ssh_public_key_id: &str,
        user_name: &str,
    ) -> SSHPublicKeyMetadata {
        SSHPublicKeyMetadata {
            user_name: user_name.to_string(),
            ssh_public_key_id: ssh_public_key_id.to_string(),
            status: self.status.clone(),
            upload_date: self.upload_date,
        }
    }
}

impl IamState {
    /// Mint the next SSH public key's `SSHPublicKeyId`.
    ///
    /// Unique and stable for the same reason every other minted id of this account is: the
    /// counter is bijectively mixed into the id, and the id is minted once and then stored
    /// with the key, so "the unique identifier for the SSH public key"
    /// (`SSHPublicKey$SSHPublicKeyId`) is what every later read reports.
    pub fn next_ssh_public_key_id(&mut self) -> String {
        self.ssh_public_key_id_seq += 1;
        unique_id(SSH_PUBLIC_KEY_ID_PREFIX, self.ssh_public_key_id_seq)
    }

    /// How many SSH public keys the user keyed by `user_key` holds.
    ///
    /// This is the measure the SSH-public-keys-per-user quota
    /// ([`MAX_SSH_PUBLIC_KEYS_PER_USER`]) is taken against.
    pub fn ssh_public_keys_held_by(&self, user_key: &str) -> usize {
        self.ssh_public_keys
            .values()
            .filter(|record| record.user_key == user_key)
            .count()
    }

    /// Whether the user keyed by `user_key` already holds the key whose MATERIAL
    /// `subject_public_key_info` is.
    ///
    /// Sameness is of the KEY, not of the text that carried it: two bodies that decode to
    /// the same public key are the same key however they were encoded, which is what
    /// recognises "the SSH public key is already associated with the specified IAM user"
    /// (`DuplicateSSHPublicKeyException`). The question is asked of THAT user's keys alone,
    /// exactly as that sentence scopes it, so a key another user holds is not mistaken for
    /// one this user holds — and a key this user holds is recognised however many other
    /// users hold it too.
    pub fn ssh_public_key_held_by(&self, user_key: &str, subject_public_key_info: &[u8]) -> bool {
        self.ssh_public_keys.values().any(|record| {
            record.user_key == user_key
                && record.subject_public_key_info == subject_public_key_info
        })
    }

    /// Store `record` for the user keyed by `user_key`, under a freshly minted id, and
    /// answer that id.
    ///
    /// The one write that adds an SSH public key, so the id a key is held under is minted
    /// in exactly one place.
    pub fn store_ssh_public_key(&mut self, record: SshPublicKeyRecord) -> String {
        let ssh_public_key_id = self.next_ssh_public_key_id();
        self.ssh_public_keys
            .insert(ssh_public_key_id.clone(), record);
        ssh_public_key_id
    }

    /// The key `ssh_public_key_id` names AMONG the keys the user keyed by `user_key` holds.
    ///
    /// An SSH public key is reachable only through the user it is associated with, so an id
    /// the account never minted and an id that belongs to a DIFFERENT user are one answer:
    /// that user holds no such key. The one lookup every operation that addresses a single
    /// key by id goes through.
    pub fn held_ssh_public_key(
        &self,
        user_key: &str,
        ssh_public_key_id: &str,
    ) -> Option<&SshPublicKeyRecord> {
        self.ssh_public_keys
            .get(ssh_public_key_id)
            .filter(|record| record.user_key == user_key)
    }
}

/// What a `SSHPublicKeyBody` carries, as the upload reads it.
///
/// The three answers are the three the operation's modelled refusals distinguish: an
/// encoding neither public-key family recognises, a recognised encoding carrying no usable
/// key, and a key.
#[derive(Debug, Clone)]
pub enum SshPublicKeyMaterial {
    /// The body is in neither of the encodings the upload names — not an OpenSSH public
    /// key line and not a PEM structure — which is
    /// `UnrecognizedPublicKeyEncodingException`.
    UnrecognizedEncoding,
    /// The body is in one of those encodings but carries no public key the upload can
    /// take: it does not decode, it is not an RSA key, or the key is shorter than
    /// [`MIN_SSH_PUBLIC_KEY_BITS`]. That is `InvalidPublicKeyException`, "the public key is
    /// malformed or otherwise invalid".
    Invalid,
    /// A key the upload takes.
    Key {
        /// Its DER `SubjectPublicKeyInfo`, which is what tells it from every other key.
        subject_public_key_info: Vec<u8>,
        /// Its `Fingerprint`: "the MD5 message digest of the SSH public key"
        /// (`SSHPublicKey$Fingerprint`) — the digest of the key in the SSH public key
        /// format itself (the `ssh-rsa` key blob of RFC 4253 section 6.6), as the
        /// colon-separated lowercase hex pairs a hex fingerprint is published in. That is
        /// the value `ssh-keygen -l -E md5` prints for the same key, so one key always
        /// fingerprints to one string whichever encoding carried it.
        fingerprint: String,
    },
}

/// The key the body `ssh_public_key_body` carries.
///
/// Both encodings the upload names are read — "the public key must be encoded in ssh-rsa
/// format or PEM format" — and each is reduced to the one structure that identifies the key
/// it carries, so the two spellings of one key are one key here.
pub fn read_ssh_public_key(ssh_public_key_body: &str) -> SshPublicKeyMaterial {
    let text = ssh_public_key_body.trim();
    let read = if text.starts_with(OPENSSH_RSA_TYPE) {
        openssh_rsa_key(text)
    } else if text.starts_with(PEM_BEGIN) {
        pem_rsa_key(text)
    } else {
        return SshPublicKeyMaterial::UnrecognizedEncoding;
    };
    let Some((modulus, public_exponent)) = read else {
        return SshPublicKeyMaterial::Invalid;
    };
    if integer_bits(&modulus) < MIN_SSH_PUBLIC_KEY_BITS {
        return SshPublicKeyMaterial::Invalid;
    }
    let Some(subject_public_key_info) = subject_public_key_info(&modulus, &public_exponent) else {
        return SshPublicKeyMaterial::Invalid;
    };
    let Some(key_blob) = openssh_rsa_key_blob(&modulus, &public_exponent) else {
        return SshPublicKeyMaterial::Invalid;
    };
    SshPublicKeyMaterial::Key {
        fingerprint: colon_separated_hex(&::md5::Md5::digest(&key_blob)),
        subject_public_key_info,
    }
}

/// The SSH public key blob of the RSA public key `(modulus, public_exponent)` — the key
/// itself in the SSH public key format (RFC 4253 section 6.6: the key type name, then the
/// exponent and the modulus as `mpint`s), which is the material an OpenSSH line carries
/// base64-encoded and which "the MD5 message digest of the SSH public key"
/// (`SSHPublicKey$Fingerprint`) is taken over.
///
/// Written by the crate that implements the format — [`ssh_key`]'s own encoding of the key
/// — rather than laid out here, so the digest is taken over the real encoding of the key and
/// not over a blob written out by hand. It is a function of the KEY alone, so both encodings
/// a body may arrive in fingerprint alike, and the comment a line may carry is no part of
/// it.
fn openssh_rsa_key_blob(modulus: &[u8], public_exponent: &[u8]) -> Option<Vec<u8>> {
    ::ssh_key::PublicKey::new(rsa_key_data(modulus, public_exponent)?, String::new())
        .to_bytes()
        .ok()
}

/// The SSH key data of the RSA public key `(modulus, public_exponent)` — the one place the
/// account turns the integers that identify a key into the key as [`ssh_key`] holds it, so
/// the blob the fingerprint is taken over and the OpenSSH line a read reports are provably
/// the same key.
fn rsa_key_data(modulus: &[u8], public_exponent: &[u8]) -> Option<::ssh_key::public::KeyData> {
    Some(::ssh_key::public::KeyData::Rsa(
        ::ssh_key::public::RsaPublicKey {
            e: ::ssh_key::Mpint::from_positive_bytes(public_exponent).ok()?,
            n: ::ssh_key::Mpint::from_positive_bytes(modulus).ok()?,
        },
    ))
}

/// The `(modulus, public exponent)` of the RSA key the OpenSSH public key line `text`
/// carries, or `None` when the line is not one.
fn openssh_rsa_key(text: &str) -> Option<(Vec<u8>, Vec<u8>)> {
    let key = ::ssh_key::PublicKey::from_openssh(text).ok()?;
    match key.key_data() {
        ::ssh_key::public::KeyData::Rsa(rsa) => Some((
            rsa.n.as_positive_bytes()?.to_vec(),
            rsa.e.as_positive_bytes()?.to_vec(),
        )),
        // A readable OpenSSH key of another algorithm: the encoding is recognised, but
        // `ssh-rsa` is the format the upload names.
        _ => None,
    }
}

/// The same pair out of a PEM body, in either of the two labels a public key is written
/// under, or `None` when the body carries no RSA public key.
fn pem_rsa_key(text: &str) -> Option<(Vec<u8>, Vec<u8>)> {
    let (label, document) = ::pkcs1::der::Document::from_pem(text).ok()?;
    let der = document.as_bytes();
    match label {
        PEM_LABEL_RSA_PUBLIC_KEY => {
            let key = ::pkcs1::RsaPublicKey::from_der(der).ok()?;
            Some((
                key.modulus.as_bytes().to_vec(),
                key.public_exponent.as_bytes().to_vec(),
            ))
        }
        PEM_LABEL_SUBJECT_PUBLIC_KEY_INFO => spki_rsa_key(der),
        // Every other PEM structure (a certificate, a private key): recognised framing,
        // but no public key the upload can take.
        _ => None,
    }
}

/// The same pair out of the DER `SubjectPublicKeyInfo` `der`, or `None` when that structure
/// carries no RSA public key.
///
/// The one reading of the structure [`subject_public_key_info`] writes, so what a body is
/// reduced to on the way in is read back the same way on the way out.
fn spki_rsa_key(der: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let info = ::pkcs8::spki::SubjectPublicKeyInfoRef::from_der(der).ok()?;
    if info.algorithm.oid != ::pkcs1::ALGORITHM_OID {
        return None;
    }
    let key = ::pkcs1::RsaPublicKey::from_der(info.subject_public_key.as_bytes()?).ok()?;
    Some((
        key.modulus.as_bytes().to_vec(),
        key.public_exponent.as_bytes().to_vec(),
    ))
}

/// Which of the encodings `encodingType` names the body `ssh_public_key_body` is written
/// in, or `None` when it is in neither.
///
/// Read by the crates that implement those formats, exactly as [`read_ssh_public_key`]
/// reads the material itself: the question is what the text IS, not what it looks like.
fn body_encoding(ssh_public_key_body: &str) -> Option<&'static str> {
    let text = ssh_public_key_body.trim();
    if ::ssh_key::PublicKey::from_openssh(text).is_ok() {
        Some(ENCODING_SSH)
    } else if ::pkcs1::der::Document::from_pem(text).is_ok() {
        Some(ENCODING_PEM)
    } else {
        None
    }
}

/// The OpenSSH `ssh-rsa` public key line for the key the DER `SubjectPublicKeyInfo`
/// `subject_public_key_info` holds — the one-line form "ssh-rsa format" names — written by
/// the crate that implements the format.
///
/// No comment is written: a comment names the key's origin on the machine that generated
/// it, and the account holds a key rather than a line.
fn openssh_public_key_line(subject_public_key_info: &[u8]) -> Option<String> {
    let (modulus, public_exponent) = spki_rsa_key(subject_public_key_info)?;
    ::ssh_key::PublicKey::new(rsa_key_data(&modulus, &public_exponent)?, String::new())
        .to_openssh()
        .ok()
}

/// The X.509 `SubjectPublicKeyInfo` PEM for the same structure — the PEM form of a public
/// key (RFC 7468 section 13), what `ssh-keygen -e -m PKCS8` writes — framed by the crate
/// that implements PEM, with the line ending that framing defines.
fn subject_public_key_info_pem(subject_public_key_info: &[u8]) -> Option<String> {
    ::pkcs1::der::Document::try_from(subject_public_key_info)
        .ok()?
        .to_pem(
            PEM_LABEL_SUBJECT_PUBLIC_KEY_INFO,
            ::pkcs1::der::pem::LineEnding::LF,
        )
        .ok()
}

/// The DER `SubjectPublicKeyInfo` of the RSA public key `(modulus, public_exponent)` — the
/// X.509 public key `openssl rsa -pubout -outform DER` writes, which identifies the key
/// whichever encoding carried it and which the PEM form of the key is written out from.
///
/// The structure is written by the crates that implement the standards it comes from: the
/// PKCS#1 `RSAPublicKey` body by [`pkcs1`] (RFC 8017), under the `rsaEncryption` algorithm
/// identifier that crate publishes, and the `SubjectPublicKeyInfo` that carries it by
/// `spki` (RFC 5280). EC2 derives the same structure for the fingerprint of an imported RSA
/// key pair; that reading is kept there rather than shared, because hoisting it would put
/// the PKCS#1, SPKI, MD5 and OpenSSH crates into the dependency closure of every service
/// that uses the shared crate for anything at all.
fn subject_public_key_info(modulus: &[u8], public_exponent: &[u8]) -> Option<Vec<u8>> {
    let body = ::pkcs1::RsaPublicKey {
        modulus: ::pkcs1::der::asn1::UintRef::new(modulus).ok()?,
        public_exponent: ::pkcs1::der::asn1::UintRef::new(public_exponent).ok()?,
    }
    .to_der()
    .ok()?;
    ::pkcs8::spki::SubjectPublicKeyInfo {
        algorithm: ::pkcs8::spki::AlgorithmIdentifier {
            oid: ::pkcs1::ALGORITHM_OID,
            parameters: Some(::pkcs1::der::asn1::AnyRef::NULL),
        },
        subject_public_key: ::pkcs1::der::asn1::BitStringRef::new(0, &body).ok()?,
    }
    .to_der()
    .ok()
}

/// How many bits the big-endian unsigned integer `bytes` holds — an RSA modulus's key
/// length, which is what the minimum the upload states is taken against.
fn integer_bits(bytes: &[u8]) -> usize {
    let Some(leading) = bytes.iter().position(|byte| *byte != 0) else {
        return 0;
    };
    let Some(top) = bytes.get(leading) else {
        return 0;
    };
    bytes
        .len()
        .saturating_sub(leading)
        .saturating_mul(8)
        .saturating_sub(top.leading_zeros() as usize)
}

/// `digest` as the colon-separated lowercase hex pairs a hex fingerprint is published in
/// (`dc:0c:bd:34:…`), which is the rendering `publicKeyFingerprintType`'s `^[:\w]+$`
/// admits.
///
/// The hex coding itself is [`data_encoding`]'s, the crate this service already spells
/// base32 with; only the pairing is done here.
fn colon_separated_hex(digest: &[u8]) -> String {
    let hex = ::data_encoding::HEXLOWER.encode(digest);
    let pairs: Vec<&str> = (0..hex.len())
        .step_by(2)
        .map(|at| &hex[at..(at + 2).min(hex.len())])
        .collect();
    pairs.join(":")
}

/// The user an SSH public key operation addresses, as the account keys users.
///
/// "The name of the IAM user … if none is specified, the UserName field is determined
/// implicitly based on the Amazon Web Services access key used to sign the request"
/// (`ListSSHPublicKeys$UserName`), resolved in the one place this service resolves an
/// absent `UserName`.
pub fn ssh_public_key_user_key(
    store: &IamState,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    user_name: Option<&str>,
) -> Option<String> {
    match user_name {
        Some(user_name) => Some(name_key(user_name)),
        None => super::authorization::caller_identity::implicit_user_key(store, ctx),
    }
}

/// The record an upload stores for a key it has read: active, dated now, and holding both
/// the body as it arrived and the material that identifies it.
pub fn uploaded_ssh_public_key(
    user_key: String,
    ssh_public_key_body: String,
    subject_public_key_info: Vec<u8>,
    fingerprint: String,
) -> SshPublicKeyRecord {
    SshPublicKeyRecord {
        user_key,
        ssh_public_key_body,
        subject_public_key_info,
        fingerprint,
        status: CREDENTIAL_STATUS_ACTIVE.to_string(),
        upload_date: super::now(),
    }
}
