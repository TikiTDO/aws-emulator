// SPDX-License-Identifier: CC0-1.0

//! What the pure readings of a custom domain name's front door make of a record and a
//! path: which base path mapping answers a request, and where a record's mutual-TLS
//! truststore is.
//!
//! The wire behaviour — the handshake, the mutual-TLS outcomes and the stage that answers —
//! is driven end to end over a real socket in
//! `tests/all/regional_custom_domain_front_door.rs`.

use super::*;

use crate::state::{BasePathMappingEntry, DomainNameEntry, NONE_BASE_PATH};

/// One row of the base-path table: the request path, and the api, stage and remaining path
/// it reaches (`None` for a path no mapping answers).
type MatchRow<'a> = (&'a str, Option<(&'a str, Option<&'a str>, &'a str)>);

/// One row of the truststore table: the `(truststoreUri, truststoreVersion)` the record
/// carries, and the `(bucket, key, version)` it addresses.
type TruststoreRow<'a> = (
    Option<(&'a str, Option<&'a str>)>,
    Option<(&'a str, &'a str, Option<&'a str>)>,
);

/// A record carrying one mapping per `(base path, api, stage)` row.
fn record_with(mappings: &[(&str, &str, Option<&str>)]) -> DomainNameEntry {
    let mut record = DomainNameEntry {
        domain_name: "api.example.com".to_string(),
        ..DomainNameEntry::default()
    };
    for (sequence, (base_path, api, stage)) in mappings.iter().enumerate() {
        record.base_path_mappings.insert(
            (*base_path).to_string(),
            BasePathMappingEntry {
                base_path: (*base_path).to_string(),
                rest_api_id: (*api).to_string(),
                stage: stage.map(str::to_string),
                sequence: sequence as u64,
            },
        );
    }
    record
}

/// RULE: the longest base path that is a prefix of the request path ON A SEGMENT BOUNDARY
/// serves it, the `(none)` mapping serves every path no other mapping claims, and a path no
/// mapping answers to reaches no api.
#[test]
fn the_longest_base_path_on_a_segment_boundary_serves_the_path() {
    let record = record_with(&[
        ("shop", "api-shop", Some("live")),
        ("shop/admin", "api-admin", Some("beta")),
        ("stageless", "api-stageless", None),
    ]);
    // The rows span what the match generalises over: an exact base path, a path under it, a
    // longer base path winning over a shorter prefix, a segment that merely starts with a
    // base path, a mapping that names no stage, a path carrying a relative segment in either
    // spelling, and a path no mapping claims.
    let cases: &[MatchRow<'_>] = &[
        ("/shop", Some(("api-shop", Some("live"), ""))),
        ("/shop/items/9", Some(("api-shop", Some("live"), "items/9"))),
        ("/shop/admin/users", Some(("api-admin", Some("beta"), "users"))),
        ("/shopping", None),
        ("/stageless/live/x", Some(("api-stageless", None, "live/x"))),
        ("/shop/./items", None),
        ("/shop/../shop/items", None),
        ("/shop/%2e%2e/items", None),
        ("/shop/%2E./items", None),
        ("/", None),
    ];
    for (path, expected) in cases {
        let matched = matching_mapping(&record, path).map(|(mapping, remainder)| {
            (
                mapping.rest_api_id.clone(),
                mapping.stage.clone(),
                remainder.to_string(),
            )
        });
        let rendered = matched
            .as_ref()
            .map(|(api, stage, remainder)| (api.as_str(), stage.as_deref(), remainder.as_str()));
        assert_eq!(
            rendered, *expected,
            "case {path:?}: expected {expected:?}, got {rendered:?}"
        );
    }
}

/// RULE: the `(none)` mapping answers every path no other mapping claims, whole.
#[test]
fn the_none_mapping_answers_every_other_path() {
    let record = record_with(&[
        (NONE_BASE_PATH, "api-root", Some("live")),
        ("shop", "api-shop", Some("live")),
    ]);
    let cases: &[(&str, &str, &str)] = &[
        ("/", "api-root", ""),
        ("/pets", "api-root", "pets"),
        ("/pets/9", "api-root", "pets/9"),
        ("/shop/items", "api-shop", "items"),
    ];
    for (path, api, remainder) in cases {
        let matched = matching_mapping(&record, path);
        let rendered = matched.map(|(mapping, left)| (mapping.rest_api_id.clone(), left.to_string()));
        assert_eq!(
            rendered.as_ref().map(|(api, left)| (api.as_str(), left.as_str())),
            Some((*api, *remainder)),
            "case {path:?}: expected {api} and {remainder:?}, got {rendered:?}"
        );
    }
}

/// RULE: a record's truststore is the object the `s3://bucket/key` of `truststoreUri`
/// addresses, at the version the record names; a value that is not such a URL addresses no
/// object and enforces nothing.
#[test]
fn a_truststore_uri_reads_as_the_object_it_addresses() {
    // The rows span what the reading generalises over: a bucket and key with and without a
    // version, a nested key, a blank version, a URI with no key, another scheme, and a
    // record carrying no mutual-TLS configuration at all.
    let cases: &[TruststoreRow<'_>] = &[
        (
            Some(("s3://bundles/truststore.pem", Some("v7"))),
            Some(("bundles", "truststore.pem", Some("v7"))),
        ),
        (
            Some(("s3://bundles/truststore.pem", None)),
            Some(("bundles", "truststore.pem", None)),
        ),
        (
            Some(("s3://bundles/certs/2024/ca.pem", Some("  "))),
            Some(("bundles", "certs/2024/ca.pem", None)),
        ),
        (Some(("s3://bundles", Some("v1"))), None),
        (Some(("s3://bundles/", Some("v1"))), None),
        (Some(("https://bundles/truststore.pem", None)), None),
        (None, None),
    ];
    for (configured, expected) in cases {
        let mut record = record_with(&[]);
        record.mutual_tls_authentication = configured.map(|(uri, version)| {
            crate::ops::types::MutualTlsAuthentication {
                truststore_uri: Some(uri.to_string()),
                truststore_version: version.map(str::to_string),
                truststore_warnings: None,
            }
        });
        let located = truststore_location(&record)
            .map(|location| (location.bucket, location.key, location.version));
        let rendered = located.as_ref().map(|(bucket, key, version)| {
            (bucket.as_str(), key.as_str(), version.as_deref())
        });
        assert_eq!(
            rendered, *expected,
            "case {configured:?}: expected {expected:?}, got {rendered:?}"
        );
    }
}

/// RULE: trust is a property of the chain the client presented — a leaf that chains to a CA
/// of the truststore is trusted, and one signed by any other CA, or presented against a
/// truststore holding no CA at all, is not.
#[test]
fn a_client_certificate_is_trusted_only_where_it_chains_to_the_truststore() {
    let authority = certificate_authority("trusted-ca.example.com");
    let other = certificate_authority("other-ca.example.net");
    let trusted = leaf_signed_by(&authority, "client.example.com");
    let untrusted = leaf_signed_by(&other, "client.example.com");
    let anchors = certificates_in(authority.pem.as_bytes());
    assert_eq!(
        anchors.len(),
        1,
        "case the truststore bundle: expected one CA certificate, got {}",
        anchors.len()
    );
    // The rows span what the verdict generalises over: a chain to the truststore's CA, a
    // chain to another CA, a chain presented against an empty truststore, and no chain at
    // all.
    let cases: &[(&str, bool, bool, ClientVerdict)] = &[
        ("a leaf the truststore's CA signed", true, true, ClientVerdict::Trusted),
        ("a leaf another CA signed", true, false, ClientVerdict::Untrusted),
        ("a trusted leaf against an empty truststore", false, true, ClientVerdict::Untrusted),
        ("no certificate at all", true, false, ClientVerdict::Untrusted),
    ];
    for (case, with_anchors, trusted_leaf, expected) in cases {
        let store = if *with_anchors { anchors.clone() } else { Vec::new() };
        let presented = match (*case, *trusted_leaf) {
            ("no certificate at all", _) => Vec::new(),
            (_, true) => certificates_in(trusted.as_bytes()),
            (_, false) => certificates_in(untrusted.as_bytes()),
        };
        let verdict = trust_verdict(&store, &presented);
        assert_eq!(
            verdict, *expected,
            "case {case}: expected {expected:?}, got {verdict:?}"
        );
    }
}

/// A certificate authority a case signs client certificates with: its PEM and the key pair
/// that signs.
struct Authority {
    pem: String,
    certificate: ::rcgen::Certificate,
    key: ::rcgen::KeyPair,
}

/// A real CA certificate, generated by `rcgen` — something other than the code under test.
fn certificate_authority(name: &str) -> Authority {
    let key = ::rcgen::KeyPair::generate().expect("a key pair");
    let mut params =
        ::rcgen::CertificateParams::new(vec![name.to_string()]).expect("certificate parameters");
    params.is_ca = ::rcgen::IsCa::Ca(::rcgen::BasicConstraints::Unconstrained);
    params.key_usages = vec![
        ::rcgen::KeyUsagePurpose::KeyCertSign,
        ::rcgen::KeyUsagePurpose::CrlSign,
        ::rcgen::KeyUsagePurpose::DigitalSignature,
    ];
    let certificate = params.self_signed(&key).expect("a self-signed CA");
    Authority {
        pem: certificate.pem(),
        certificate,
        key,
    }
}

/// A real client certificate for `name`, signed by `authority`, as the PEM a client
/// presents.
fn leaf_signed_by(authority: &Authority, name: &str) -> String {
    let key = ::rcgen::KeyPair::generate().expect("a key pair");
    let mut params =
        ::rcgen::CertificateParams::new(vec![name.to_string()]).expect("certificate parameters");
    params.is_ca = ::rcgen::IsCa::ExplicitNoCa;
    params.extended_key_usages = vec![::rcgen::ExtendedKeyUsagePurpose::ClientAuth];
    params
        .signed_by(&key, &authority.certificate, &authority.key)
        .expect("a signed client certificate")
        .pem()
}
