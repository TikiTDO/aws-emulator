// SPDX-License-Identifier: CC0-1.0

//! What a `CreateCertificateAuthority` idempotency token is honoured FOR.
//!
//! The RULE under test: an idempotency token distinguishes RETRIES of one call, so it
//! replays the CA the first call minted only for a request carrying the SAME
//! `CertificateAuthorityConfiguration`; the same token sent with a different configuration
//! is a different call and creates a second, wholly independent CA with its own ARN, each
//! keeping the configuration it was asked for. A token sent with no configuration change
//! stays idempotent however many times it repeats.
//!
//! The rows differ along what the rule generalises over: WHICH part of the configuration
//! differs (the key algorithm, the signing algorithm, one subject attribute, an added
//! subject attribute, a custom attribute), and the repeats that change nothing (the same
//! configuration again, and the same configuration with a member whose value is the one it
//! already held).
//!
//! The harness (the engine, the router, the awsJson1_1 call) is
//! [`crate::private_ca_lifecycle::Emulator`].

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::private_ca_lifecycle::{string, Emulator, REGION};

/// The token every row of a test reuses: one string, so what decides the answer is the
/// request beside it.
const TOKEN: &str = "one-token-for-every-row";

/// A `CreateCertificateAuthority` request for `configuration`, under [`TOKEN`].
fn create_with(configuration: &Value) -> Value {
    json!({
        "CertificateAuthorityConfiguration": configuration,
        "CertificateAuthorityType": "ROOT",
        "IdempotencyToken": TOKEN,
    })
}

/// The configuration the first call of each test makes.
fn first_configuration() -> Value {
    json!({
        "KeyAlgorithm": "RSA_2048",
        "SigningAlgorithm": "SHA256WITHRSA",
        "Subject": {"CommonName": "ca-a"},
    })
}

/// The `CertificateAuthorityConfiguration` the service reports for the CA at `arn`.
async fn reported_configuration(emulator: &Emulator, case: &str, arn: &str) -> Value {
    let described = emulator
        .ok(
            case,
            "DescribeCertificateAuthority",
            &json!({"CertificateAuthorityArn": arn}),
        )
        .await;
    described
        .get("CertificateAuthority")
        .and_then(|authority| authority.get("CertificateAuthorityConfiguration"))
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no configuration in {described:?}"))
}

/// One token, a materially different configuration each time: one CA per configuration,
/// each describing itself as the configuration it was created for.
#[tokio::test]
async fn one_token_with_a_different_configuration_creates_a_ca_of_its_own() {
    let emulator = Emulator::new();

    let first = create_with(&first_configuration());
    let first_arn = string(
        &emulator.ok("the first CA", "CreateCertificateAuthority", &first).await,
        "CertificateAuthorityArn",
        "the first CA",
    );

    // Each row differs from the first configuration in ONE way, and each way is a
    // different kind of difference: the key, the signature, a changed attribute, an added
    // attribute, a custom attribute.
    let rows: [(&str, Value); 5] = [
        (
            "a different key algorithm",
            json!({
                "KeyAlgorithm": "RSA_4096",
                "SigningAlgorithm": "SHA256WITHRSA",
                "Subject": {"CommonName": "ca-a"},
            }),
        ),
        (
            "a different signing algorithm",
            json!({
                "KeyAlgorithm": "RSA_2048",
                "SigningAlgorithm": "SHA512WITHRSA",
                "Subject": {"CommonName": "ca-a"},
            }),
        ),
        (
            "a different common name",
            json!({
                "KeyAlgorithm": "RSA_2048",
                "SigningAlgorithm": "SHA256WITHRSA",
                "Subject": {"CommonName": "ca-b"},
            }),
        ),
        (
            "one more subject attribute",
            json!({
                "KeyAlgorithm": "RSA_2048",
                "SigningAlgorithm": "SHA256WITHRSA",
                "Subject": {"CommonName": "ca-a", "Organization": "acme-widgets"},
            }),
        ),
        (
            "a custom subject attribute",
            json!({
                "KeyAlgorithm": "RSA_2048",
                "SigningAlgorithm": "SHA256WITHRSA",
                "Subject": {
                    "CommonName": "ca-a",
                    "CustomAttributes": [{"ObjectIdentifier": "2.5.4.44", "Value": "III"}],
                },
            }),
        ),
    ];

    let mut minted: BTreeSet<String> = BTreeSet::new();
    minted.insert(first_arn.clone());
    for (case, configuration) in &rows {
        let body = create_with(configuration);
        let created = emulator.ok(case, "CreateCertificateAuthority", &body).await;
        let arn = string(&created, "CertificateAuthorityArn", case);
        assert_ne!(
            arn, first_arn,
            "case {case:?}: a token reused for another configuration must mint a CA of its \
             own, got the first CA's ARN {arn:?}"
        );
        assert!(
            minted.insert(arn.clone()),
            "case {case:?}: every configuration must get its own ARN, got {arn:?} again"
        );

        // Each handle describes the configuration IT was created for, so no call landed on
        // another row's CA.
        let reported = reported_configuration(&emulator, case, &arn).await;
        assert_eq!(
            reported.get("KeyAlgorithm"),
            configuration.get("KeyAlgorithm"),
            "case {case:?}: expected the row's key algorithm, got {reported:?}"
        );
        assert_eq!(
            reported.get("SigningAlgorithm"),
            configuration.get("SigningAlgorithm"),
            "case {case:?}: expected the row's signing algorithm, got {reported:?}"
        );
        assert_eq!(
            reported.get("Subject"),
            configuration.get("Subject"),
            "case {case:?}: expected the row's subject, got {reported:?}"
        );
    }

    // The first CA is untouched by any of it: it still describes its own configuration.
    let first_reported = reported_configuration(&emulator, "the first CA", &first_arn).await;
    assert_eq!(
        first_reported,
        first_configuration(),
        "the first CA must keep its own configuration, got {first_reported:?}"
    );

    // And the emulator holds exactly one CA per configuration asked for.
    let listed = emulator
        .ok("the CAs the emulator holds", "ListCertificateAuthorities", &json!({}))
        .await;
    let held: BTreeSet<String> = listed
        .get("CertificateAuthorities")
        .and_then(Value::as_array)
        .unwrap_or(&Vec::new())
        .iter()
        .map(|authority| string(authority, "Arn", "the CAs the emulator holds"))
        .collect();
    assert_eq!(
        held, minted,
        "expected one CA per configuration asked for: {minted:?}, got {held:?}"
    );
}

/// The other half of the rule: a repeat of the SAME request under one token is idempotent,
/// however many times it repeats and whichever members it spells out.
#[tokio::test]
async fn one_token_with_the_same_configuration_replays_the_first_ca() {
    let emulator = Emulator::new();

    let first_arn = string(
        &emulator
            .ok(
                "the first CA",
                "CreateCertificateAuthority",
                &create_with(&first_configuration()),
            )
            .await,
        "CertificateAuthorityArn",
        "the first CA",
    );

    // Repeats that ask for the same CA: the identical request twice over, and the same
    // request carrying members whose values are the ones the first call already implied.
    let repeats: [(&str, Value); 4] = [
        ("the identical request", create_with(&first_configuration())),
        ("the identical request once more", create_with(&first_configuration())),
        (
            "the same configuration with an explicit empty custom attribute list absent",
            create_with(&json!({
                "KeyAlgorithm": "RSA_2048",
                "SigningAlgorithm": "SHA256WITHRSA",
                "Subject": {"CommonName": "ca-a"},
            })),
        ),
        (
            "the same configuration beside a tag set, which is not the configuration",
            {
                let mut body = create_with(&first_configuration());
                body["Tags"] = json!([{"Key": "owner", "Value": "platform"}]);
                body
            },
        ),
    ];

    for (case, body) in &repeats {
        let created = emulator.ok(case, "CreateCertificateAuthority", body).await;
        let arn = string(&created, "CertificateAuthorityArn", case);
        assert_eq!(
            arn, first_arn,
            "case {case:?}: a repeat of the same request must replay {first_arn:?}, got {arn:?}"
        );
    }

    // One CA for all of them.
    let listed = emulator
        .ok("the CAs the emulator holds", "ListCertificateAuthorities", &json!({}))
        .await;
    let held = listed
        .get("CertificateAuthorities")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();
    assert_eq!(
        held, 1,
        "a replayed token must leave exactly one CA, got {held} in {listed:?}"
    );
}

/// One token honoured for TWO requests remembers both: reusing it for another
/// configuration mints a CA of its own and does NOT displace the record of the request the
/// token was first honoured for, so the first request stays replayable for the rest of its
/// window — and so does each later one.
///
/// The rows are the other configurations the token is reused with, differing in which part
/// of the configuration changed; after each one, the FIRST request is repeated verbatim and
/// has to answer the first CA again.
#[tokio::test]
async fn a_token_reused_for_another_configuration_keeps_replaying_the_first() {
    let emulator = Emulator::new();

    let first_arn = string(
        &emulator
            .ok(
                "the first CA",
                "CreateCertificateAuthority",
                &create_with(&first_configuration()),
            )
            .await,
        "CertificateAuthorityArn",
        "the first CA",
    );

    let rows: [(&str, Value); 3] = [
        (
            "a different key algorithm",
            json!({
                "KeyAlgorithm": "RSA_4096",
                "SigningAlgorithm": "SHA256WITHRSA",
                "Subject": {"CommonName": "ca-a"},
            }),
        ),
        (
            "a different common name",
            json!({
                "KeyAlgorithm": "RSA_2048",
                "SigningAlgorithm": "SHA256WITHRSA",
                "Subject": {"CommonName": "ca-b"},
            }),
        ),
        (
            "a different signing algorithm",
            json!({
                "KeyAlgorithm": "RSA_2048",
                "SigningAlgorithm": "SHA512WITHRSA",
                "Subject": {"CommonName": "ca-a"},
            }),
        ),
    ];

    let mut minted: BTreeSet<String> = BTreeSet::new();
    minted.insert(first_arn.clone());
    for (case, configuration) in &rows {
        let body = create_with(configuration);
        let other = string(
            &emulator.ok(case, "CreateCertificateAuthority", &body).await,
            "CertificateAuthorityArn",
            case,
        );
        assert!(
            minted.insert(other.clone()),
            "case {case:?}: another configuration must mint a CA of its own, got {other:?} \
             again"
        );

        // The first request, verbatim, after the token was reused for this row: the record
        // of the first configuration is still the token's answer for it.
        let replayed = string(
            &emulator
                .ok(
                    case,
                    "CreateCertificateAuthority",
                    &create_with(&first_configuration()),
                )
                .await,
            "CertificateAuthorityArn",
            case,
        );
        assert_eq!(
            replayed, first_arn,
            "case {case:?}: expected the first request to keep replaying {first_arn:?}, got \
             {replayed:?}"
        );

        // And this row's own request replays its own CA, so the token remembers both.
        let again = string(
            &emulator.ok(case, "CreateCertificateAuthority", &body).await,
            "CertificateAuthorityArn",
            case,
        );
        assert_eq!(
            again, other,
            "case {case:?}: expected this configuration to replay {other:?}, got {again:?}"
        );
    }

    // One CA per distinct configuration asked for, and no more: no repeat minted anything.
    let listed = emulator
        .ok("the CAs the emulator holds", "ListCertificateAuthorities", &json!({}))
        .await;
    let held: BTreeSet<String> = listed
        .get("CertificateAuthorities")
        .and_then(Value::as_array)
        .unwrap_or(&Vec::new())
        .iter()
        .map(|authority| string(authority, "Arn", "the CAs the emulator holds"))
        .collect();
    assert_eq!(
        held, minted,
        "expected one CA per configuration asked for: {minted:?}, got {held:?}"
    );
}

/// A token is not a handle to a resource: two DIFFERENT tokens carrying the same
/// configuration are two calls, so each mints its own CA.
#[tokio::test]
async fn two_tokens_with_one_configuration_mint_two_cas() {
    let emulator = Emulator::new();

    let mut minted: BTreeSet<String> = BTreeSet::new();
    for token in ["token-one", "token-two", "token-three"] {
        let mut body = create_with(&first_configuration());
        body["IdempotencyToken"] = json!(token);
        let created = emulator.ok(token, "CreateCertificateAuthority", &body).await;
        let arn = string(&created, "CertificateAuthorityArn", token);
        assert!(
            minted.insert(arn.clone()),
            "case {token:?}: a token of its own must mint a CA of its own, got {arn:?} again"
        );
    }
    assert_eq!(
        minted.len(),
        3,
        "expected one CA per token, got {minted:?}"
    );
}

/// A token is not a handle to a resource across regions either: a private CA belongs to the
/// region its ARN names, so one token carrying one configuration replays only the CA of the
/// region the call was made in, and a client of another region is minted a CA of its own.
///
/// The rows are the regions a client signs for; the region is read from the call's credential
/// scope, as every ARN this service mints is.
#[tokio::test]
async fn one_token_in_another_region_mints_a_ca_of_its_own() {
    let emulator = Emulator::new();
    let body = create_with(&first_configuration());

    let mut minted: BTreeSet<String> = BTreeSet::new();
    for region in [REGION, "us-east-1", "ap-southeast-2"] {
        let case = format!("a client of {region}");
        let (status, created) = emulator
            .call_in_region(region, "CreateCertificateAuthority", &body)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the create to answer 200, got {status} {created:?}"
        );
        let arn = string(&created, "CertificateAuthorityArn", &case);
        let prefix = format!("arn:aws:acm-pca:{region}:");
        assert!(
            arn.starts_with(&prefix),
            "case {case:?}: expected a CA ARN under {prefix:?}, got {arn:?}"
        );
        assert!(
            minted.insert(arn.clone()),
            "case {case:?}: a region of its own must mint a CA of its own, got {arn:?} again"
        );

        // The retry the token is for: the same call from the same region replays that CA.
        let (_, again) = emulator
            .call_in_region(region, "CreateCertificateAuthority", &body)
            .await;
        let replayed = string(&again, "CertificateAuthorityArn", &case);
        assert_eq!(
            replayed, arn,
            "case {case:?}: expected the retry to replay {arn:?}, got {replayed:?}"
        );
    }
    assert_eq!(
        minted.len(),
        3,
        "expected one CA per region the token was presented in, got {minted:?}"
    );
}
