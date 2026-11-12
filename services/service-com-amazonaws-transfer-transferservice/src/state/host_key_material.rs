// SPDX-License-Identifier: CC0-1.0

//! The private key MATERIAL a caller supplies for a host key, and the properties this
//! service reads off it.
//!
//! The containers and the key classes are the ones the service's own documentation tells
//! a caller to produce: `CreateServer`'s `HostKey` documents `ssh-keygen -t rsa -b 2048
//! -N "" -m PEM`, `-t ecdsa -b 256 -N "" -m PEM` and `-t ed25519`, and `ImportHostKey`'s
//! `HostKeyBody` "accepts RSA, ECDSA, and ED25519 keys". So the material may arrive in
//! the OpenSSH container the plain `ssh-keygen` invocation writes or in a PEM one (the
//! PKCS#1 and SEC1 structures `-m PEM` writes, and the PKCS#8 structure `-m PKCS8`
//! wraps either in), and it may hold an RSA, an ECDSA or an Ed25519 key. Each names ONE
//! key, and the algorithm, the fingerprint and the public half reported for it are that
//! key's own — read through the crates that implement each structure, never from the
//! container's text.

/// The first line of the OpenSSH private-key container (`ssh-keygen`'s own armour).
pub(super) const OPENSSH_PRIVATE_KEY_BEGIN: &str = "-----BEGIN OPENSSH PRIVATE KEY-----";

/// The last line of that container.
pub(super) const OPENSSH_PRIVATE_KEY_END: &str = "-----END OPENSSH PRIVATE KEY-----";

/// How many Base64 columns [`normalised_openssh_private_key`] wraps the payload at.
///
/// The width OpenSSH itself writes, and the ONE width [`ssh_key`]'s container decoder
/// accepts: it reads the payload through a line-wrapped Base64 reader fixed at this
/// width, so an armour wrapped at any other column count is rejected by it even though
/// the key inside is the same key.
pub(super) const OPENSSH_ARMOUR_COLUMNS: usize = 70;

/// The OpenSSH private-key container around `material`'s payload, re-emitted in the
/// one armour [`ssh_key`]'s decoder reads — or `None` when `material` carries no such
/// container at all.
///
/// The container's LAYOUT is not part of the key: what identifies the key is the Base64
/// payload between the two boundary lines, so the line width that payload is wrapped at
/// (OpenSSH writes 70 columns, other tools 64 or none at all), the line ending it uses
/// and any surrounding blank space all name the SAME key and must be read identically.
/// This strips the boundary lines and every whitespace character of the payload and
/// re-wraps what is left at [`OPENSSH_ARMOUR_COLUMNS`]; every other character is kept
/// exactly as supplied, so material whose payload is not a key still fails to parse
/// rather than being silently repaired.
pub(super) fn normalised_openssh_private_key(material: &str) -> Option<String> {
    let inner = material
        .trim()
        .strip_prefix(OPENSSH_PRIVATE_KEY_BEGIN)?
        .trim_end()
        .strip_suffix(OPENSSH_PRIVATE_KEY_END)?;
    let payload: Vec<char> = inner.chars().filter(|c| !c.is_whitespace()).collect();
    let mut normalised = String::with_capacity(material.len() + OPENSSH_ARMOUR_COLUMNS);
    normalised.push_str(OPENSSH_PRIVATE_KEY_BEGIN);
    normalised.push('\n');
    for line in payload.chunks(OPENSSH_ARMOUR_COLUMNS) {
        normalised.extend(line.iter());
        normalised.push('\n');
    }
    normalised.push_str(OPENSSH_PRIVATE_KEY_END);
    normalised.push('\n');
    Some(normalised)
}

/// What this service keeps of ONE host key: the three properties it reports about the
/// key, every one of them computed from the key's own material.
///
/// The private half is never part of it — nothing in this emulator can use it and no
/// modelled member returns it — while the public half is, because that is the half a
/// client connecting to the server is shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostKeyMaterial {
    /// The Base64 SHA256 fingerprint, in the `SHA256:<base64>` form `ssh-keygen -l`
    /// prints.
    pub fingerprint: String,
    /// The SSH algorithm name (`ssh-rsa`, `ssh-ed25519`, `ecdsa-sha2-nistp256/384/521`).
    pub key_type: String,
    /// The public half, in the `key-type key-body` form `ssh-keygen` writes.
    pub public_key: String,
}

/// The genuine properties of private key MATERIAL a caller supplied, when the material
/// is a key the emulator can read.
///
/// Both documented containers are read: the OpenSSH one (through
/// [`normalised_openssh_private_key`], so whichever way the tool that wrote it wrapped
/// the armour, the same key reads the same way) and the PEM one `ssh-keygen -m PEM`
/// writes. The container is not part of the key: the same key in either form reports the
/// same algorithm, fingerprint and public half.
///
/// `None` for material in a format the emulator cannot read, and for a payload that is
/// not a key at all: there is then nothing to compute a fingerprint, an algorithm name
/// or a public half from — and none of them may be invented for material nothing parsed.
pub fn read_host_key(material: &str) -> Option<HostKeyMaterial> {
    read_openssh_private_key(material).or_else(|| read_pem_private_key(material))
}

/// The properties of a key supplied in the OpenSSH private-key container.
fn read_openssh_private_key(material: &str) -> Option<HostKeyMaterial> {
    let normalised = normalised_openssh_private_key(material)?;
    let key = ::ssh_key::PrivateKey::from_openssh(&normalised).ok()?;
    Some(host_key_material(&key))
}

/// The PEM label of the PKCS#1 RSA private key `ssh-keygen -t rsa -m PEM` writes.
const PEM_LABEL_RSA_PRIVATE_KEY: &str = "RSA PRIVATE KEY";

/// The PEM label of the SEC1 EC private key `ssh-keygen -t ecdsa -m PEM` writes.
const PEM_LABEL_EC_PRIVATE_KEY: &str = "EC PRIVATE KEY";

/// The PEM label of the PKCS#8 private key `ssh-keygen -m PKCS8` writes, which wraps
/// one of the structures above behind an algorithm identifier.
const PEM_LABEL_PRIVATE_KEY: &str = "PRIVATE KEY";

/// The `rsaEncryption` algorithm identifier a PKCS#8 container names an RSA key with
/// (RFC 8017), whose `privateKey` octets are then a PKCS#1 `RSAPrivateKey`.
const OID_RSA_ENCRYPTION: &str = "1.2.840.113549.1.1.1";

/// The `id-ecPublicKey` identifier a PKCS#8 container names an EC key with (RFC 5480),
/// whose `privateKey` octets are then a SEC1 `ECPrivateKey`.
const OID_EC_PUBLIC_KEY: &str = "1.2.840.10045.2.1";

/// The `id-Ed25519` identifier a PKCS#8 container names an Ed25519 key with (RFC 8410),
/// whose `privateKey` octets are then the 32-byte seed in an OCTET STRING.
const OID_ED25519: &str = "1.3.101.112";

/// The properties of a key supplied in a PEM private-key container.
///
/// The public half of a key is a function of the private structure the container holds:
/// an RSA key's modulus and public exponent, an EC key's curve and point, an Ed25519
/// key's seed. So the SSH public key — and with it the algorithm name and the
/// fingerprint `ssh-keygen -l` prints — is computed from the supplied key's own numbers.
/// `None` for a label this service does not implement and for DER that does not decode.
fn read_pem_private_key(material: &str) -> Option<HostKeyMaterial> {
    let (label, der) = ::pkcs1::pem::decode_vec(material.trim().as_bytes()).ok()?;
    match label {
        PEM_LABEL_RSA_PRIVATE_KEY => {
            read_pkcs1_rsa(&::pkcs1::RsaPrivateKey::try_from(der.as_slice()).ok()?)
        }
        PEM_LABEL_EC_PRIVATE_KEY => read_sec1_ec(&der),
        PEM_LABEL_PRIVATE_KEY => read_pkcs8_der(&der),
        _ => None,
    }
}

/// The properties of the key a PKCS#8 `PrivateKeyInfo` holds — the encoding a PKCS#8
/// container carries and a key generator hands back. The algorithm identifier says which
/// structure the `privateKey` octets are, so each documented kind is read through the
/// same reader a bare container of that kind goes through.
fn read_pkcs8_der(der: &[u8]) -> Option<HostKeyMaterial> {
    let info = ::pkcs8::PrivateKeyInfo::try_from(der).ok()?;
    match info.algorithm.oid.to_string().as_str() {
        OID_RSA_ENCRYPTION => {
            read_pkcs1_rsa(&::pkcs1::RsaPrivateKey::try_from(info.private_key).ok()?)
        }
        OID_EC_PUBLIC_KEY => read_sec1_ec(info.private_key),
        OID_ED25519 => read_ed25519_seed(info.private_key),
        _ => None,
    }
}

/// The properties of an RSA key, read off the PKCS#1 structure's own numbers: the SSH
/// public key of an RSA key IS its modulus and public exponent.
fn read_pkcs1_rsa(rsa: &::pkcs1::RsaPrivateKey<'_>) -> Option<HostKeyMaterial> {
    let public = ::ssh_key::public::RsaPublicKey {
        e: ::ssh_key::Mpint::from_positive_bytes(rsa.public_exponent.as_bytes()).ok()?,
        n: ::ssh_key::Mpint::from_positive_bytes(rsa.modulus.as_bytes()).ok()?,
    };
    let key = ::ssh_key::PublicKey::new(::ssh_key::public::KeyData::Rsa(public), "");
    Some(public_key_material(&key))
}

/// The properties of an EC key, read off the SEC1 structure's own curve and point: the
/// SSH public key of an ECDSA key IS the named curve and the uncompressed point, and the
/// curve decides which `ecdsa-sha2-nistp*` algorithm the key is.
fn read_sec1_ec(der: &[u8]) -> Option<HostKeyMaterial> {
    let ec = ::sec1::EcPrivateKey::try_from(der).ok()?;
    let public = ::ssh_key::public::EcdsaPublicKey::from_sec1_bytes(ec.public_key?).ok()?;
    let key = ::ssh_key::PublicKey::new(::ssh_key::public::KeyData::Ecdsa(public), "");
    Some(public_key_material(&key))
}

/// The properties of an Ed25519 key, read off the seed the container holds: the SSH
/// public key of an Ed25519 key is derived from that seed.
fn read_ed25519_seed(der: &[u8]) -> Option<HostKeyMaterial> {
    use ::pkcs8::der::Decode;

    let seed = ::pkcs8::der::asn1::OctetStringRef::from_der(der).ok()?;
    let seed: [u8; 32] = seed.as_bytes().try_into().ok()?;
    let private = ::ssh_key::private::Ed25519PrivateKey::from_bytes(&seed);
    let keypair = ::ssh_key::private::Ed25519Keypair::from(private);
    let key = ::ssh_key::PublicKey::new(::ssh_key::public::KeyData::Ed25519(keypair.public), "");
    Some(public_key_material(&key))
}

/// The fixed `Message` the service answers private key MATERIAL it cannot read with.
///
/// It reports the defect — the key's format could not be read — and nothing else: no
/// member name, and no expected encoding, so the one sentence answers every form of
/// unreadable material and every member that carries key material
/// ([`read_host_key`]'s callers). The accepted encodings are the operation's
/// documentation's business, not this refusal's.
pub const INVALID_HOST_KEY_FORMAT: &str = "Invalid host key format";

/// Mint a host key for a new server.
///
/// AWS generates a host key for a server whose host key it manages — an RSA key, the
/// kind its own documented generation command writes first (`ssh-keygen -t rsa -b 2048
/// -N "" -m PEM`), so a reader reports `ssh-rsa` for it — and reports that key's
/// fingerprint, "equivalent to the output of the `ssh-keygen -l -f` command", i.e.
/// OpenSSH's `SHA256:<base64 of the SHA256 of the public key blob>`. So this GENERATES a
/// real RSA 2048 key pair and reads that key's own properties off it; every value is
/// computed from real key material rather than invented. The key is per-server and
/// per-run, like the real service's, and its private half is not kept: the emulator
/// serves no SSH transport, so nothing can use it, and nothing may return it.
///
/// `None` when the generator or the encoding it hands back is not one this service can
/// read: a server whose host key is of another algorithm than the one the service makes
/// is not a server this service has, so the caller reports the failure as the server
/// fault it is rather than substituting a key of a different class.
pub fn mint_host_key() -> Option<HostKeyMaterial> {
    use ::aws_lc_rs::encoding::AsDer;

    let key = ::aws_lc_rs::rsa::KeyPair::generate(::aws_lc_rs::rsa::KeySize::Rsa2048).ok()?;
    let der = key.as_der().ok()?;
    read_pkcs8_der(der.as_ref())
}

/// The three properties this service keeps of one parsed private key. The ONE place
/// they are read off a key, so a minted key and an imported one are described alike.
pub(super) fn host_key_material(key: &::ssh_key::PrivateKey) -> HostKeyMaterial {
    public_key_material(key.public_key())
}

/// The same three properties, read off the PUBLIC half alone.
///
/// Every one of them is a property of the public key: the fingerprint is the hash of its
/// wire encoding, the algorithm name is its own, and the public half IS it. So a key
/// supplied in a container that carries the private numbers and a key read from the
/// public ones are described through this one place.
fn public_key_material(key: &::ssh_key::PublicKey) -> HostKeyMaterial {
    HostKeyMaterial {
        fingerprint: key.fingerprint(::ssh_key::HashAlg::Sha256).to_string(),
        key_type: key.algorithm().as_str().to_string(),
        // An OpenSSH public key encodes from the key data alone; the fallback is the
        // empty string, which no reader can mistake for a key, rather than a value this
        // service made up for a key that would not encode.
        public_key: key.to_openssh().unwrap_or_default(),
    }
}
