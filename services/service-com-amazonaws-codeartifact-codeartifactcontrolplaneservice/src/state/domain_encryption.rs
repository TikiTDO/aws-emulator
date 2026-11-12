// SPDX-License-Identifier: CC0-1.0

//! The KMS key a domain is encrypted with, resolved against KMS.
//!
//! A domain "is used to encrypt content stored in a domain"
//! (`smithy_query(query="doc",shape="CreateDomainRequest",member="encryptionKey")`) with
//! ONE key, and `DomainDescription$encryptionKey` reports "the ARN of an Key Management
//! Service (KMS) key associated with a domain" — the key itself, as its ARN, whatever
//! spelling the create used. That resolution is made ONCE, where the domain is created,
//! and recorded on the domain, so every later read (`DescribeDomain`, `ListDomains`,
//! `DeleteDomain`) reports the same key without asking KMS again.
//!
//! Two kinds of reference reach here:
//!
//! * the caller NAMED a key — "an encryption key can be a key ID, a key Amazon Resource
//!   Name (ARN), a key alias, or a key alias ARN", four spellings of one key, so the key
//!   the domain reports is the one KMS resolves the reference to;
//! * the caller named NONE, which asks for the Amazon Web Services managed key this
//!   service owns in the account and Region ([`AWS_MANAGED_KEY_ALIAS`]). Its alias is a
//!   PREDEFINED Amazon Web Services alias — one that "exists in every account and Region"
//!   carrying no target key until something asks for it, when "KMS associates the alias
//!   with an Amazon Web Services managed key" — so asking for it is what gives it a key,
//!   and every later domain created without a key in that account and Region resolves the
//!   same alias to the SAME key.
//!
//! Only a CREATE asks this: resolving the predefined alias materialises the managed key
//! behind it, which a read must not do.

use ::service_com_amazonaws_kms_trentservice::state as kms_store;

/// The alias of the Amazon Web Services managed key CodeArtifact encrypts a domain with
/// when the create names no key: `aws/<service-name>` is the form of such an alias, under
/// this service's own name.
pub const AWS_MANAGED_KEY_ALIAS: &str = "alias/aws/codeartifact";

/// The value `DomainDescription$encryptionKey` records for a domain being created in
/// `region`, from the `encryptionKey` the request named (`requested`), or from no key
/// at all.
///
/// A reference KMS holds a key for resolves to THAT key's ARN, so a key id, an alias name
/// and an alias ARN all record the one key they name. A reference KMS holds no key for is
/// recorded as the caller wrote it: the key is the caller's own, named in an account or a
/// key store this emulator does not keep, and nothing here invents an ARN for it or
/// refuses a create AWS serves.
///
/// `None` only when the request named no key and no managed key could be associated for
/// the Region — there is then no key to report, and `encryptionKey` is modelled optional.
///
/// LOCKING: takes the KMS store's lock and no other, and a handler asks it while holding
/// this service's own guard. No KMS operation reaches into the CodeArtifact store, so the
/// two can never be taken in opposite orders, and the work done under both is one lookup.
pub fn domain_encryption_key<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    requested: Option<&str>,
) -> Option<String> {
    // No KMS store in this process (an engine that is not the shared in-memory one): the
    // key cannot be resolved, so the caller's own reference is recorded unchanged and a
    // create that named none reports no key rather than one nothing holds.
    let Some(store) = kms_store::service_state(engine) else {
        return requested.map(str::to_string);
    };
    let reference = requested.unwrap_or(AWS_MANAGED_KEY_ALIAS);
    let mut store = store.lock();
    if kms_store::is_predefined_aws_alias(reference) {
        return store
            .associate_predefined_aws_alias(region, reference)
            .map(|key| key.arn.clone())
            .or_else(|| requested.map(str::to_string));
    }
    match store.resolve(region, reference) {
        kms_store::KeyRef::Found(key) => Some(key.arn.clone()),
        _ => Some(reference.to_string()),
    }
}
