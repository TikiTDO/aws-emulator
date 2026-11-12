// SPDX-License-Identifier: CC0-1.0

//! Which certificates a directory can register: the FUNCTION the certificate performs has
//! to be one the directory's own type offers.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router (`crate::harness`), so nothing here reaches into
//! the state struct and an answer that is not computed from persisted state cannot pass.
//!
//! The rule under test: `RegisterCertificate` stores a registration only when the
//! directory's `DirectoryType` offers the function the `Type` member names — client-side
//! LDAPS for the types `EnableLDAPS` serves, and client (smart card) authentication for AD
//! Connector — and refuses every other pairing with `UnsupportedOperationException`,
//! storing nothing, so `ListCertificates` stays as it was. It is asserted over the WHOLE
//! grid of the modelled `CertificateType` catalog (both values and the request that names
//! none, which takes the documented default) against every `DirectoryType` this service
//! creates.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ad_connector::{connect_directory, minimal_connect};
use crate::certificates::{certificate_pem, TYPE_CLIENT_CERT_AUTH, TYPE_CLIENT_LDAPS};
use crate::directory_lifecycle::{create_directory, minimal_create, PAST_ONE_SPAN};
use crate::harness::{call, ok, service};
use crate::microsoft_ad::{create_microsoft_ad, minimal_microsoft_ad};

/// The sentence Directory Service refuses a client-authentication request against a
/// directory type that does not offer it with (`state::CLIENT_AUTHENTICATION_UNSUPPORTED`),
/// as the recorded refusal of such a call words it.
const CLIENT_AUTHENTICATION_UNSUPPORTED: &str =
    "Client authentication operations are not supported for this Directory Type.";

/// The same error class's sentence for the other half of the gate
/// (`state::LDAPS_UNSUPPORTED`): a certificate that secures LDAP calls, asked for on a
/// directory type that serves no LDAPS endpoint.
const LDAPS_UNSUPPORTED: &str = "LDAPS operations are not supported for this Directory Type.";

/// The suffix `crate::state::refusal` appends to every message of this service: the
/// correlation handle, whose value is per call.
const REQUEST_ID_SEPARATOR: &str = " : RequestId: ";

/// One directory type under test, and how to create a directory of it.
enum DirectoryKind {
    /// `CreateDirectory`: a Simple AD directory.
    SimpleAd,
    /// `CreateMicrosoftAD`: an AWS Managed Microsoft AD directory.
    ManagedMicrosoftAd,
    /// `ConnectDirectory`: an AD Connector.
    AdConnector,
}

impl DirectoryKind {
    /// A directory of this kind, in service (past the stage transition its create owes).
    async fn create(&self, router: &MultiServiceRouter, name: &str, case: &str) -> String {
        let id = match self {
            Self::SimpleAd => create_directory(router, minimal_create(name), case).await,
            Self::ManagedMicrosoftAd => {
                create_microsoft_ad(router, minimal_microsoft_ad(name), case).await
            }
            Self::AdConnector => connect_directory(router, minimal_connect(name), case).await,
        };
        tokio::time::sleep(PAST_ONE_SPAN).await;
        id
    }
}

/// The certificate ids `ListCertificates` reports for `directory_id`.
///
/// Shared with the sibling suite `certificate_writes_during_a_build`, so both read a
/// directory's inventory the one way.
pub(crate) async fn listed(
    router: &MultiServiceRouter,
    directory_id: &str,
    case: &str,
) -> Vec<String> {
    let answer = ok(
        router,
        "ListCertificates",
        json!({ "DirectoryId": directory_id }),
        case,
    )
    .await;
    answer["CertificatesInfo"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected CertificatesInfo, got {answer}"))
        .iter()
        .map(|entry| entry["CertificateId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// RULE: a certificate is registered only for a directory whose type offers the function
/// the certificate performs; every other pairing is refused with
/// `UnsupportedOperationException` and stores nothing.
#[tokio::test]
async fn a_certificate_is_registered_only_where_its_function_is_offered() {
    let router = service();

    // The grid: every directory type this service creates against the whole modelled
    // `CertificateType` catalog plus the request that names none. `accepted` is whether
    // the type offers that function — LDAPS for Managed Microsoft AD and AD Connector
    // (the types `EnableLDAPS` serves), client authentication for AD Connector.
    let rows: [(&str, DirectoryKind, Option<&str>, bool); 9] = [
        (
            "a Simple AD directory, LDAPS named",
            DirectoryKind::SimpleAd,
            Some(TYPE_CLIENT_LDAPS),
            false,
        ),
        (
            "a Simple AD directory, no type named",
            DirectoryKind::SimpleAd,
            None,
            false,
        ),
        (
            "a Simple AD directory, client authentication",
            DirectoryKind::SimpleAd,
            Some(TYPE_CLIENT_CERT_AUTH),
            false,
        ),
        (
            "a Managed Microsoft AD directory, LDAPS named",
            DirectoryKind::ManagedMicrosoftAd,
            Some(TYPE_CLIENT_LDAPS),
            true,
        ),
        (
            "a Managed Microsoft AD directory, no type named",
            DirectoryKind::ManagedMicrosoftAd,
            None,
            true,
        ),
        (
            "a Managed Microsoft AD directory, client authentication",
            DirectoryKind::ManagedMicrosoftAd,
            Some(TYPE_CLIENT_CERT_AUTH),
            false,
        ),
        (
            "an AD Connector, LDAPS named",
            DirectoryKind::AdConnector,
            Some(TYPE_CLIENT_LDAPS),
            true,
        ),
        (
            "an AD Connector, no type named",
            DirectoryKind::AdConnector,
            None,
            true,
        ),
        (
            "an AD Connector, client authentication",
            DirectoryKind::AdConnector,
            Some(TYPE_CLIENT_CERT_AUTH),
            true,
        ),
    ];

    for (index, (case, kind, requested_type, accepted)) in rows.iter().enumerate() {
        let directory_id = kind
            .create(&router, &format!("certificates{index}.example.com"), case)
            .await;
        let (pem, _) = certificate_pem(Some("gate.example.com"), (2031, 4, 5));
        let mut body = json!({ "DirectoryId": &directory_id, "CertificateData": pem });
        if let Some(wanted) = requested_type {
            body["Type"] = json!(wanted);
        }

        let (status, parsed) = call(&router, "RegisterCertificate", body).await;
        let inventory = listed(&router, &directory_id, case).await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the registration to be accepted, got {status} with \
                 {parsed}"
            );
            let certificate_id = parsed["CertificateId"].as_str().unwrap_or_default();
            assert_eq!(
                inventory,
                vec![certificate_id.to_string()],
                "case {case:?}: expected the directory to hold exactly the accepted \
                 registration, got {inventory:?}"
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {case:?}: expected the registration to be refused, got {status} with {parsed}"
        );
        let wire_type = parsed["__type"].as_str().unwrap_or_default();
        assert!(
            wire_type.contains("UnsupportedOperationException"),
            "case {case:?}: expected UnsupportedOperationException, got {parsed}"
        );
        assert!(
            inventory.is_empty(),
            "case {case:?}: expected the refused registration to store nothing, got \
             {inventory:?}"
        );
        // The function the request asked for decides which of the two sentences the
        // error class states: neither quotes the request, because the directory type the
        // call was made against is what the caller already has.
        let message = parsed["Message"].as_str().unwrap_or_default();
        let (text, _) = message
            .split_once(REQUEST_ID_SEPARATOR)
            .unwrap_or((message, ""));
        let expected = if *requested_type == Some(TYPE_CLIENT_CERT_AUTH) {
            CLIENT_AUTHENTICATION_UNSUPPORTED
        } else {
            LDAPS_UNSUPPORTED
        };
        assert_eq!(
            text, expected,
            "case {case:?}: expected that refusal's own sentence, got {message:?}"
        );
        assert!(
            message.contains(REQUEST_ID_SEPARATOR),
            "case {case:?}: expected the refusal to carry the call's request id, got \
             {message:?}"
        );
    }
}

/// RULE: a refusal of this family states its error class's own sentence, so a client that
/// branches on the message reads what AWS words for that class rather than emulator prose.
#[tokio::test]
async fn the_certificate_refusals_state_their_error_classes_own_sentences() {
    let router = service();
    let directory_id = DirectoryKind::ManagedMicrosoftAd
        .create(&router, "sentences.example.com", "the sentences")
        .await;
    let (pem, _) = certificate_pem(Some("sentences.example.com"), (2033, 8, 9));
    let registered = ok(
        &router,
        "RegisterCertificate",
        json!({ "DirectoryId": &directory_id, "CertificateData": &pem }),
        "the sentences",
    )
    .await;
    let certificate_id = registered["CertificateId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // (case, the operation, its body, the sentence that error class states)
    let rows: [(&str, &str, Value, String); 3] = [
        (
            "the same certificate again",
            "RegisterCertificate",
            json!({ "DirectoryId": &directory_id, "CertificateData": &pem }),
            "The certificate has already been registered into the system.".to_string(),
        ),
        (
            // The invalid-certificate refusal names the registration it could not perform
            // and the directory the request named it for.
            "data that is not a certificate",
            "RegisterCertificate",
            json!({ "DirectoryId": &directory_id, "CertificateData": "not a certificate" }),
            format!(
                "Certificate is not valid. Could not register the certificate with the \
                 directory {directory_id}"
            ),
        ),
        (
            // The not-found refusal is the one of this family that names both ids the
            // lookup was made with: which certificate, of which directory, is not there.
            "an id no registration answers to",
            "DescribeCertificate",
            json!({ "DirectoryId": &directory_id, "CertificateId": "c-0123456789" }),
            format!("Certificate c-0123456789 does not exist for directoryId: {directory_id}"),
        ),
    ];

    for (case, operation, body, expected) in rows {
        let (status, parsed) = call(&router, operation, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {parsed}"
        );
        let message = parsed["Message"].as_str().unwrap_or_default();
        let (text, _) = message
            .split_once(REQUEST_ID_SEPARATOR)
            .unwrap_or((message, ""));
        assert_eq!(
            text, expected,
            "case {case:?}: expected the error class's own sentence, got {message:?}"
        );
    }

    // The registration the rows above did not disturb is still the directory's only one.
    let inventory = listed(&router, &directory_id, "the sentences").await;
    assert_eq!(
        inventory,
        vec![certificate_id],
        "case \"the sentences\": expected the refusals to change nothing, got {inventory:?}"
    );
}
