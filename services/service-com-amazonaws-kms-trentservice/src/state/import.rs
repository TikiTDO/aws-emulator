// SPDX-License-Identifier: CC0-1.0

//! Importing key material: the wrapping key pair `GetParametersForImport` hands out,
//! the import token that names it, and the unwrapping `ImportKeyMaterial` performs.
//!
//! One concern of [`super`]'s store, in its own file: the records and helpers here are
//! reached as `crate::state::…` exactly as they were before, because the parent
//! re-exports them.

use super::*;

/// The `WrappingKeySpec` values this service generates a wrapping key pair for, from
/// `smithy_query(query="shape",shape="WrappingKeySpec")`. `SM2` is the fourth value
/// and is not one of these: see [`wrapping_key_size`].
pub const WRAPPING_KEY_SPEC_RSA_2048: &str = "RSA_2048";
/// See [`WRAPPING_KEY_SPEC_RSA_2048`].
pub const WRAPPING_KEY_SPEC_RSA_3072: &str = "RSA_3072";
/// See [`WRAPPING_KEY_SPEC_RSA_2048`].
pub const WRAPPING_KEY_SPEC_RSA_4096: &str = "RSA_4096";

/// The RSA key size a `WrappingKeySpec` names, or `None` for a spec that is not an RSA
/// key pair this service generates.
///
/// The accepted specs are the catalog above; the modulus size is read from the spec's
/// own name through [`rsa_key_size`], the one place a modulus size becomes a key size.
fn wrapping_key_size(wrapping_key_spec: &str) -> Option<::aws_lc_rs::rsa::KeySize> {
    match wrapping_key_spec {
        WRAPPING_KEY_SPEC_RSA_2048 | WRAPPING_KEY_SPEC_RSA_3072 | WRAPPING_KEY_SPEC_RSA_4096 => {
            rsa_key_size(wrapping_key_spec.strip_prefix("RSA_")?.parse().ok()?)
        }
        _ => None,
    }
}

/// Whether this service can generate a wrapping key pair of `wrapping_key_spec`.
pub fn generates_wrapping_key(wrapping_key_spec: &str) -> bool {
    wrapping_key_size(wrapping_key_spec).is_some()
}

/// A freshly generated wrapping key pair of the kind `wrapping_key_spec` names: the
/// public half as X.509 `SubjectPublicKeyInfo` DER, which is what the response carries,
/// and the private half as PKCS#8 DER, which is what this store keeps.
///
/// The RSA key pair comes from `aws-lc-rs` (`aws_lc_rs::rsa`) rather than from anything
/// written here, and the public half really is the public half of the private one, so
/// key material wrapped with it is material `ImportKeyMaterial` can open.
///
/// `None` means the platform refused to generate a key pair; the caller answers a
/// modelled server error rather than returning a public key with no private half.
pub fn mint_wrapping_key(wrapping_key_spec: &str) -> Option<(Vec<u8>, Vec<u8>)> {
    use ::aws_lc_rs::encoding::AsDer;

    let key_size = wrapping_key_size(wrapping_key_spec)?;
    let private_key = ::aws_lc_rs::rsa::PrivateDecryptingKey::generate(key_size).ok()?;
    let public_key = private_key.public_key().as_der().ok()?.as_ref().to_vec();
    let private_key_pkcs8 = private_key.as_der().ok()?.as_ref().to_vec();
    Some((public_key, private_key_pkcs8))
}

// The matching UNWRAP of key material a caller sealed under the public half of a
// wrapping key pair is [`unwrap_key_material`] below.

/// Open key material a caller wrapped under the public half of `parameters`' wrapping
/// key pair, with the wrapping algorithm those parameters were issued for.
///
/// The RSA-OAEP decryption comes from `aws-lc-rs` (`aws_lc_rs::rsa`), under the private
/// half this store kept when the public half was handed out, so what comes back is
/// exactly the material the caller wrapped. An `RSA_AES` wrapping has a second step:
/// what the RSA public key encrypted is an ephemeral AES key, and that key wrapped the
/// material — see [`Wrapping`].
///
/// `None` means the bytes are not key material those parameters can open: either the
/// wrapping algorithm is one this service does not unwrap (see
/// [`unwraps_wrapping_algorithm`]) or the unwrapping failed.
pub fn unwrap_key_material(parameters: &ImportParameters, wrapped: &[u8]) -> Option<Vec<u8>> {
    let wrapping = wrapping_of(&parameters.wrapping_algorithm)?;
    let private_key =
        ::aws_lc_rs::rsa::PrivateDecryptingKey::from_pkcs8(&parameters.private_key_pkcs8).ok()?;
    let key_size_bytes = private_key.key_size_bytes();
    let oaep = ::aws_lc_rs::rsa::OaepPrivateDecryptingKey::new(private_key).ok()?;
    // What the RSA public key encrypted is the whole material for an RSAES wrapping and
    // the ephemeral AES key alone for an `RSA_AES` one, in which case it occupies exactly
    // one RSA block and the AES-wrapped material is the rest.
    let (sealed, aes_wrapped) = match wrapping {
        Wrapping::Direct(_) => (wrapped, None),
        Wrapping::AesKeyWrap(_) => {
            if wrapped.len() <= key_size_bytes {
                return None;
            }
            let (block, rest) = wrapped.split_at(key_size_bytes);
            (block, Some(rest))
        }
    };
    let mut opened = vec![0u8; key_size_bytes];
    let len = oaep
        .decrypt(wrapping.oaep_algorithm(), sealed, &mut opened, None)
        .ok()?
        .len();
    opened.truncate(len);
    match aes_wrapped {
        None => Some(opened),
        Some(aes_wrapped) => aes_key_unwrap(&opened, aes_wrapped),
    }
}

/// Open key material wrapped with the ephemeral AES key `kek` under AES key wrap with
/// padding, which is the second step of an `RSA_AES` wrapping.
///
/// "Generate a 256-bit AES symmetric key", "encrypt your key material with the AES key"
/// and "encrypt the AES key with the public key", then concatenate the two
/// (`smithy_query(query="doc",shape="GetParametersForImportRequest",member="WrappingAlgorithm")`),
/// so the material is unwrapped with the AES key the RSA block carried. The key wrap
/// comes from `aws-lc-rs` (`aws_lc_rs::key_wrap`), not from anything written here.
///
/// `None` means the bytes are not material wrapped with that key.
fn aes_key_unwrap(kek: &[u8], wrapped: &[u8]) -> Option<Vec<u8>> {
    use ::aws_lc_rs::key_wrap::KeyWrapPadded;

    let kek = ::aws_lc_rs::key_wrap::AesKek::new(&::aws_lc_rs::key_wrap::AES_256, kek).ok()?;
    let mut opened = vec![0u8; wrapped.len()];
    let len = kek.unwrap_with_padding(wrapped, &mut opened).ok()?.len();
    opened.truncate(len);
    Some(opened)
}

/// Whether this service can unwrap key material wrapped with `wrapping_algorithm`.
pub fn unwraps_wrapping_algorithm(wrapping_algorithm: &str) -> bool {
    wrapping_of(wrapping_algorithm).is_some()
}

/// How a caller wrapped the key material it is importing, which is what the
/// `AlgorithmSpec` of the parameters it names states.
#[derive(Debug, Clone, Copy)]
enum Wrapping {
    /// The material itself is encrypted under the RSA public key: "for RSAES wrapping
    /// algorithms, you encrypt your key material directly with the RSA public key from
    /// KMS", with these OAEP parameters.
    Direct(&'static ::aws_lc_rs::rsa::OaepAlgorithm),
    /// An ephemeral AES key is encrypted under the RSA public key with these OAEP
    /// parameters, and the material is wrapped with that AES key: "for RSA_AES wrapping
    /// algorithms, you encrypt your key material with an AES symmetric key that you
    /// generate, then encrypt your AES key with the RSA public key from KMS".
    AesKeyWrap(&'static ::aws_lc_rs::rsa::OaepAlgorithm),
}

impl Wrapping {
    /// The OAEP parameters the RSA step uses, which both forms of wrapping carry.
    fn oaep_algorithm(&self) -> &'static ::aws_lc_rs::rsa::OaepAlgorithm {
        match self {
            Self::Direct(algorithm) | Self::AesKeyWrap(algorithm) => algorithm,
        }
    }
}

/// The wrapping an `AlgorithmSpec` names, or `None` for a spec that is not one this
/// service unwraps: the four RSA values of
/// `smithy_query(query="shape",shape="AlgorithmSpec")` are wrappings under the RSA key
/// pair `GetParametersForImport` handed out, while `SM2PKE` is not RSA at all.
///
/// The digest in the name is the digest of the OAEP step, for both forms
/// (`smithy_query(query="doc",shape="GetParametersForImportRequest",member="WrappingAlgorithm")`).
fn wrapping_of(wrapping_algorithm: &str) -> Option<Wrapping> {
    match wrapping_algorithm {
        ALGORITHM_SPEC_RSAES_OAEP_SHA_1 => Some(Wrapping::Direct(
            &::aws_lc_rs::rsa::OAEP_SHA1_MGF1SHA1,
        )),
        ALGORITHM_SPEC_RSAES_OAEP_SHA_256 => Some(Wrapping::Direct(
            &::aws_lc_rs::rsa::OAEP_SHA256_MGF1SHA256,
        )),
        ALGORITHM_SPEC_RSA_AES_KEY_WRAP_SHA_1 => Some(Wrapping::AesKeyWrap(
            &::aws_lc_rs::rsa::OAEP_SHA1_MGF1SHA1,
        )),
        ALGORITHM_SPEC_RSA_AES_KEY_WRAP_SHA_256 => Some(Wrapping::AesKeyWrap(
            &::aws_lc_rs::rsa::OAEP_SHA256_MGF1SHA256,
        )),
        _ => None,
    }
}

/// The `AlgorithmSpec` values a caller wraps key material with directly under the RSA
/// public key — see [`wrapping_of`].
pub const ALGORITHM_SPEC_RSAES_OAEP_SHA_1: &str = "RSAES_OAEP_SHA_1";
/// See [`ALGORITHM_SPEC_RSAES_OAEP_SHA_1`].
pub const ALGORITHM_SPEC_RSAES_OAEP_SHA_256: &str = "RSAES_OAEP_SHA_256";
/// The `AlgorithmSpec` values that wrap key material with an ephemeral AES key and that
/// AES key under the RSA public key — see [`wrapping_of`]. They are what "supported for
/// wrapping RSA and ECC key material" names
/// (`smithy_query(query="doc",shape="GetParametersForImportRequest",member="WrappingAlgorithm")`).
pub const ALGORITHM_SPEC_RSA_AES_KEY_WRAP_SHA_1: &str = "RSA_AES_KEY_WRAP_SHA_1";
/// See [`ALGORITHM_SPEC_RSA_AES_KEY_WRAP_SHA_1`].
pub const ALGORITHM_SPEC_RSA_AES_KEY_WRAP_SHA_256: &str = "RSA_AES_KEY_WRAP_SHA_256";

/// How long one wrapping key pair and its import token stay usable: "Each public key
/// and import token set is valid for 24 hours"
/// (`smithy_query(query="doc",shape="GetParametersForImport")`).
pub const IMPORT_PARAMETERS_VALIDITY: std::time::Duration =
    std::time::Duration::from_secs(24 * 60 * 60);

/// The largest number of outstanding wrapping key pairs this store holds.
///
/// A bound the EMULATOR imposes, not a quota KMS publishes: `GetParametersForImport`
/// mints a key pair per call and is reachable by any caller, so without a cap a loop
/// of valid calls grows the process without end. Past it the operation refuses with a
/// modelled server error and mints nothing.
pub const MAX_IMPORT_PARAMETERS: usize = 200;

/// One wrapping key pair `GetParametersForImport` handed out, with the import token
/// that names it.
///
/// "The public key (or 'wrapping key') of an RSA key pair that KMS generates" is
/// returned to the caller and the private half stays here, which is what lets
/// `ImportKeyMaterial` open the key material the caller wraps with it. "The public key
/// and its import token are permanently linked and must be used together"
/// (`smithy_query(query="doc",shape="GetParametersForImport")`), so the token is the
/// key of this record and the record holds everything the import must agree with: the
/// KMS key the parameters were issued for, and the wrapping algorithm the caller said
/// it would use.
///
/// `Debug` is written by hand rather than derived so the private key is REDACTED.
#[derive(Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct ImportParameters {
    /// The key id of the KMS key the parameters were issued for.
    pub key_id: String,
    /// The `AlgorithmSpec` the request named, which is the algorithm the caller
    /// wraps the key material with.
    pub wrapping_algorithm: String,
    /// The `WrappingKeySpec` the request named, which is the kind of RSA key pair
    /// that was generated.
    pub wrapping_key_spec: String,
    /// The PKCS#8 DER of the private half of the wrapping key pair. It never leaves
    /// this process: only the public half is returned, and only `ImportKeyMaterial`
    /// reads this one.
    pub private_key_pkcs8: Vec<u8>,
    /// When the pair and its token stop being usable, which is
    /// [`IMPORT_PARAMETERS_VALIDITY`] after the call that minted them.
    pub valid_to: SystemTime,
}

impl std::fmt::Debug for ImportParameters {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImportParameters")
            .field("key_id", &self.key_id)
            .field("wrapping_algorithm", &self.wrapping_algorithm)
            .field("wrapping_key_spec", &self.wrapping_key_spec)
            .field("private_key_pkcs8", &"<redacted>")
            .field("valid_to", &self.valid_to)
            .finish()
    }
}

impl ImportParameters {
    /// Whether these parameters are still usable at `now`: an import token past its
    /// `ParametersValidTo` "cannot be used in an ImportKeyMaterial request"
    /// (`smithy_query(query="doc",shape="GetParametersForImport")`).
    pub fn usable_at(&self, now: SystemTime) -> bool {
        now < self.valid_to
    }
}

/// The import token that names one [`ImportParameters`] record: opaque to the caller,
/// drawn from the OS CSPRNG so no two calls share one and no client can construct one.
///
/// `None` means the platform refused to produce entropy; the caller answers a modelled
/// server error rather than minting a predictable token.
pub fn mint_import_token() -> Option<Vec<u8>> {
    random_bytes(IMPORT_TOKEN_LEN)
}

/// How many bytes an import token carries. The value is this emulator's: `ImportToken`
/// is only "the import token to send in a subsequent ImportKeyMaterial request"
/// (`smithy_query(query="doc",shape="GetParametersForImportResponse",member="ImportToken")`)
/// with a modelled 1..6144 length and no structure.
const IMPORT_TOKEN_LEN: usize = 32;

/// The identity of one key material a caller IMPORTED into a KMS key: what tells that
/// material from any other, and the `KeyMaterialId` a reader reports for it.
///
/// It holds no key material: the digest identifies the bytes without keeping them, so a
/// key remembers which material is its own without holding material it no longer has.
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct ImportedMaterialIdentity {
    /// What identifies the material to this store, for every kind whose bytes it holds
    /// ([`KeyMaterial::digest`]).
    digest: String,
    /// The `KeyMaterialId` KMS reports for that material, or `None` for a kind of
    /// material KMS gives no identifier ([`KeyMaterial::id`]).
    key_material_id: Option<String>,
}

/// The most material identities one KMS key remembers.
///
/// A bound the EMULATOR imposes, not a quota KMS publishes: an import records the
/// identity of the material it supplied, and importing is reachable by any caller, so
/// without a cap a loop of valid imports grows this record without end. Past it the
/// OLDEST identity is forgotten — the material a key holds now, and every version it
/// retains, are recognised from the material itself
/// ([`Cmk::associates_key_material`]) whether or not the history still names them, so
/// what the bound can cost is only the memory of material the key was given long ago
/// and no longer holds.
const MAX_IMPORTED_MATERIAL_IDENTITIES: usize = 100;

impl Cmk {
    /// Remember that `material` has been imported into this key, so it stays material
    /// this key is associated with.
    ///
    /// Material already in the history is not recorded twice: a re-import of the same
    /// material is the same material, and the order the history keeps is the order each
    /// material was FIRST imported in.
    pub(super) fn remember_imported_material(&mut self, material: &KeyMaterial) {
        let Some(digest) = material.digest() else {
            return;
        };
        if self
            .imported_material_history
            .iter()
            .any(|known| known.digest == digest)
        {
            return;
        }
        self.imported_material_history.push(ImportedMaterialIdentity {
            digest,
            key_material_id: material.id(),
        });
        if self.imported_material_history.len() > MAX_IMPORTED_MATERIAL_IDENTITIES {
            self.imported_material_history.remove(0);
        }
    }

    /// Whether any key material has ever been imported into this key.
    pub fn has_imported_material_history(&self) -> bool {
        !self.imported_material_history.is_empty()
    }

    /// Whether `material` is key material that has been imported into this key before —
    /// the material this key is associated with, whether or not it still holds it.
    pub fn was_imported_material(&self, material: &KeyMaterial) -> bool {
        material.digest().is_some_and(|digest| {
            self.imported_material_history
                .iter()
                .any(|known| known.digest == digest)
        })
    }

    /// Whether `material` is this key's OWN key material: material it has been given
    /// before ([`Self::was_imported_material`]), or a version it is associated with now
    /// ([`Self::associates_key_material`]) — the material the key encrypts under, one a
    /// rotation retained, or one waiting for a rotation.
    ///
    /// It is what an import is judged against: material that is not the key's own cannot
    /// become the key's material, because "to use the KMS key in cryptographic
    /// operations again, you must reimport the same key material"
    /// (`smithy_query(query="doc",shape="DeleteImportedKeyMaterial")`).
    pub fn is_own_key_material(&self, material: &KeyMaterial) -> bool {
        self.was_imported_material(material) || self.associates_key_material(material)
    }

    /// The `KeyMaterialId` an import into this key is expected to supply the material
    /// of: the material the key is encrypting under when it holds some, and otherwise
    /// the material it was last given.
    ///
    /// `None` when no material has been imported, or when the material that was
    /// imported is of a kind KMS reports no identifier for.
    pub fn expected_imported_material_id(&self) -> Option<String> {
        self.current_key_material_id.clone().or_else(|| {
            self.imported_material_history
                .iter()
                .next_back()
                .and_then(|known| known.key_material_id.clone())
        })
    }
}
