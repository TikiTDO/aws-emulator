// SPDX-License-Identifier: CC0-1.0

//! The prerequisite `CreateConnector` resolves before it mints anything: the
//! directory registration that authorizes the Active Directory the request names.
//!
//! A connector joins a private CA to an Active Directory, and this service reaches
//! that directory only through the registration of it, so a `DirectoryId` carrying
//! no registration names a resource the service does not hold: the create is
//! refused with 404 `ResourceNotFoundException` naming the MISSING resource
//! (`ResourceType` `DirectoryRegistration`, `ResourceId` the identifier as
//! supplied) and NOTHING is persisted — which is why the reads agree afterwards.
//!
//! The rows span what the rule generalises over: the form of the directory id
//! (digits only, hex letters only, both ends of the pattern's range, mixtures), the
//! number of security groups (both modelled bounds), and whether the request
//! carries an idempotency token. Every case drives the real wire path through
//! [`super::harness`] with a `DirectoryId` the member's `@pattern`
//! `^d-[0-9a-f]{10}$` admits, so what refuses it is the handler's lookup and not the
//! boundary validator.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::directory_registration_authorization::set_deleted_due;
use super::harness::{
    call, encode_label, register_directory, router, router_with_engine, ACCOUNT, REGION,
};

/// Any past instant is already due, so the next call materialises the removal the
/// delete started instead of the case sleeping through the span.
const ALREADY_DUE: Duration = Duration::from_secs(1);

/// One row of the table: a `CreateConnector` request, differing from its
/// neighbours along the form of the directory id, the CA it names, how many
/// security groups it carries and whether it carries a `ClientToken`.
struct Row {
    case: &'static str,
    directory_id: &'static str,
    certificate_authority_arn: &'static str,
    security_group_ids: &'static [&'static str],
    client_token: Option<&'static str>,
}

/// `DirectoryId` is modelled `^d-[0-9a-f]{10}$` and `SecurityGroupIds` `length
/// 1..4` (`smithy_query(query="operation",operation="CreateConnector")`), so the
/// rows span both ends of the id's character range and both published bounds of the
/// group list. None of these directory ids is the one the issue recorded.
const ROWS: &[Row] = &[
    Row {
        case: "digits only, one security group",
        directory_id: "d-1234509876",
        certificate_authority_arn:
            "arn:aws:acm-pca:us-west-2:000000000000:certificate-authority/11111111-2222-3333-4444-555555555555",
        security_group_ids: &["sg-0123456789abcdef0"],
        client_token: None,
    },
    Row {
        case: "hex letters only, two security groups",
        directory_id: "d-cafebabead",
        certificate_authority_arn:
            "arn:aws:acm-pca:us-west-2:000000000000:certificate-authority/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
        security_group_ids: &["sg-000000000000000ab", "sg-000000000000000cd"],
        client_token: Some("token-prerequisite-hex"),
    },
    Row {
        case: "the bottom of the id range, four security groups at the modelled maximum",
        directory_id: "d-0000000000",
        certificate_authority_arn:
            "arn:aws:acm-pca:us-west-2:000000000000:certificate-authority/0f0f0f0f-1e1e-2d2d-3c3c-4b4b4b4b4b4b",
        security_group_ids: &[
            "sg-00000000000000001",
            "sg-00000000000000002",
            "sg-00000000000000003",
            "sg-00000000000000004",
        ],
        client_token: None,
    },
    Row {
        case: "the top of the id range, one security group",
        directory_id: "d-ffffffffff",
        certificate_authority_arn:
            "arn:aws:acm-pca:us-west-2:000000000000:certificate-authority/99999999-8888-7777-6666-555555555555",
        security_group_ids: &["sg-0fedcba987654321f"],
        client_token: Some("token-prerequisite-top"),
    },
    Row {
        case: "digits and hex letters mixed, two security groups",
        directory_id: "d-1a2b3c4d5e",
        certificate_authority_arn:
            "arn:aws:acm-pca:us-west-2:000000000000:certificate-authority/12121212-3434-5656-7878-909090909090",
        security_group_ids: &["sg-0a0a0a0a0a0a0a0a0", "sg-0b0b0b0b0b0b0b0b0"],
        client_token: None,
    },
];

impl Row {
    fn body(&self) -> Value {
        let mut body = json!({
            "DirectoryId": self.directory_id,
            "CertificateAuthorityArn": self.certificate_authority_arn,
            "VpcInformation": { "SecurityGroupIds": self.security_group_ids },
        });
        if let Some(token) = self.client_token {
            body["ClientToken"] = json!(token);
        }
        body
    }
}

async fn create_connector(router: &MultiServiceRouter, body: &Value) -> (u16, String, Value) {
    call(router, "POST", "/connectors", Some(body)).await
}

/// Every connector ARN `ListConnectors` serves, in the order it served them.
async fn listed_arns(router: &MultiServiceRouter) -> Vec<String> {
    let (status, code, page) = call(router, "GET", "/connectors", None).await;
    assert_eq!(
        status, 200,
        "ListConnectors must answer 200, got {status} / {code:?} / {page}"
    );
    page["Connectors"]
        .as_array()
        .map(|summaries| {
            summaries
                .iter()
                .filter_map(|summary| summary["Arn"].as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The refusal every row must read, whichever row it is: the modelled 404, the
/// modelled error class, and all three required members of it.
fn assert_refusal(case: &str, directory_id: &str, status: u16, code: &str, parsed: &Value) {
    assert_eq!(
        status, 404,
        "case {case:?}: a directory carrying no registration must be refused with 404, \
         got {status} / {code:?} / {parsed}"
    );
    assert_eq!(
        code, "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {code:?} / {parsed}"
    );
    let expected = format!("DirectoryRegistration was not found: {directory_id}.");
    assert_eq!(
        parsed["Message"].as_str(),
        Some(expected.as_str()),
        "case {case:?}: expected message {expected:?}, got {parsed}"
    );
    assert_eq!(
        parsed["ResourceId"].as_str(),
        Some(directory_id),
        "case {case:?}: the refusal must name the identifier as supplied, got {parsed}"
    );
    assert_eq!(
        parsed["ResourceType"].as_str(),
        Some("DirectoryRegistration"),
        "case {case:?}: the refusal must name the MISSING resource's type, got {parsed}"
    );
}

#[tokio::test]
async fn create_connector_refuses_a_directory_with_no_registration_and_persists_nothing() {
    let router = router();

    for row in ROWS {
        let (status, code, parsed) = create_connector(&router, &row.body()).await;
        assert_refusal(row.case, row.directory_id, status, &code, &parsed);
        assert!(
            parsed["ConnectorArn"].is_null(),
            "case {:?}: a refused create mints no ARN, got {parsed}",
            row.case
        );
    }

    // The refusals minted nothing, so the read path has nothing to serve: this is
    // the ListConnectors side of the same rule.
    let listed = listed_arns(&router).await;
    assert!(
        listed.is_empty(),
        "after {} refused creates ListConnectors must serve no connector, got {listed:?}",
        ROWS.len()
    );
}

#[tokio::test]
async fn a_registered_directory_is_what_lets_a_connector_be_created_and_read_back() {
    let router = router();
    let prefix = format!("arn:aws:pca-connector-ad:{REGION}:{ACCOUNT}:connector/");
    let mut created = Vec::new();

    for row in ROWS {
        register_directory(&router, row.directory_id).await;
        let body = row.body();
        let (status, code, parsed) = create_connector(&router, &body).await;
        assert_eq!(
            status, 202,
            "case {:?}: a registered directory must be accepted with the modelled 202, \
             got {status} / {code:?} / {parsed}",
            row.case
        );
        let arn = parsed["ConnectorArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no ConnectorArn: {parsed}", row.case))
            .to_string();
        assert!(
            arn.starts_with(&prefix),
            "case {:?}: expected a connector ARN starting {prefix}, got {arn:?}",
            row.case
        );

        // Round trip: the create is backed by state the reader serves.
        let (status, code, read) = call(
            &router,
            "GET",
            &format!("/connectors/{}", encode_label(&arn)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: GetConnector must read the connector back, got {status} / {code:?} / {read}",
            row.case
        );
        assert_eq!(
            read["Connector"]["DirectoryId"].as_str(),
            Some(row.directory_id),
            "case {:?}: the connector must carry the directory it was created for, got {read}",
            row.case
        );
        assert_eq!(
            read["Connector"]["VpcInformation"], body["VpcInformation"],
            "case {:?}: VpcInformation must round-trip, got {read}",
            row.case
        );
        created.push(arn);
    }

    let listed = listed_arns(&router).await;
    assert_eq!(
        listed, created,
        "every accepted create must be served by ListConnectors in creation order, \
         got {listed:?} for {created:?}"
    );
}

#[tokio::test]
async fn the_prerequisite_is_the_directory_the_request_names_not_a_non_empty_store() {
    let router = router();
    // One directory is registered; every OTHER row's directory still carries no
    // registration, so a store that holds registrations and connectors refuses
    // them just the same.
    let registered = &ROWS[0];
    register_directory(&router, registered.directory_id).await;
    let (status, code, parsed) = create_connector(&router, &registered.body()).await;
    assert_eq!(
        status, 202,
        "setup {:?}: the registered directory must be accepted, got {status} / {code:?} / {parsed}",
        registered.case
    );
    let accepted = parsed["ConnectorArn"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: no ConnectorArn: {parsed}"))
        .to_string();

    for row in &ROWS[1..] {
        let (status, code, parsed) = create_connector(&router, &row.body()).await;
        assert_refusal(row.case, row.directory_id, status, &code, &parsed);
    }

    let listed = listed_arns(&router).await;
    assert_eq!(
        listed,
        vec![accepted.clone()],
        "only the create whose directory was registered may be stored, got {listed:?}"
    );
}

#[tokio::test]
async fn a_directory_whose_registration_is_gone_no_longer_admits_a_connector() {
    let (router, engine) = router_with_engine();
    let row = &ROWS[2];
    let registration = register_directory(&router, row.directory_id).await;

    let (status, code, parsed) = create_connector(&router, &row.body()).await;
    assert_eq!(
        status, 202,
        "case {:?}: while the directory is registered the create is accepted, \
         got {status} / {code:?} / {parsed}",
        row.case
    );
    let before = parsed["ConnectorArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {:?}: no ConnectorArn: {parsed}", row.case))
        .to_string();

    // Deauthorize the directory, then put the removal the delete started in the
    // past: only the DUE INSTANT is moved, so the service itself is what stops
    // reporting the registration.
    let (status, code, parsed) = call(
        &router,
        "DELETE",
        &format!("/directoryRegistrations/{}", encode_label(&registration)),
        None,
    )
    .await;
    assert_eq!(
        status, 202,
        "DeleteDirectoryRegistration must answer the modelled 202, \
         got {status} / {code:?} / {parsed}"
    );
    set_deleted_due(&engine, &registration, SystemTime::now() - ALREADY_DUE);

    let mut body = row.body();
    // A fresh token, so the refusal is the prerequisite gate and not an
    // idempotent replay of the create above.
    body["ClientToken"] = json!("token-after-deauthorization");
    let (status, code, parsed) = create_connector(&router, &body).await;
    assert_refusal(row.case, row.directory_id, status, &code, &parsed);

    let listed = listed_arns(&router).await;
    assert_eq!(
        listed,
        vec![before.clone()],
        "the refused create must not have been stored, got {listed:?}"
    );
}

#[tokio::test]
async fn a_refused_create_consumes_neither_the_client_token_nor_a_connector_slot() {
    let router = router();
    let row = &ROWS[1];
    let token = row
        .client_token
        .expect("this row is the one carrying a ClientToken");
    let body = row.body();

    let (status, code, parsed) = create_connector(&router, &body).await;
    assert_refusal(row.case, row.directory_id, status, &code, &parsed);

    // The refusal recorded nothing, so the SAME token is still free: once the
    // prerequisite is there, the retry mints a connector rather than replaying a
    // create that never happened.
    register_directory(&router, row.directory_id).await;
    let (status, code, parsed) = create_connector(&router, &body).await;
    assert_eq!(
        status, 202,
        "case {token}: the retry of a refused create must be accepted once the directory \
         is registered, got {status} / {code:?} / {parsed}"
    );
    let arn = parsed["ConnectorArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {token}: no ConnectorArn: {parsed}"))
        .to_string();

    let (status, code, retried) = create_connector(&router, &body).await;
    assert_eq!(
        status, 202,
        "case {token}: a retried create must answer 202, got {status} / {code:?} / {retried}"
    );
    assert_eq!(
        retried["ConnectorArn"].as_str(),
        Some(arn.as_str()),
        "case {token}: the token must name the connector it minted, got {retried}"
    );
    let listed = listed_arns(&router).await;
    assert_eq!(
        listed,
        vec![arn.clone()],
        "one token over three calls must leave exactly one connector, got {listed:?}"
    );
}
