// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the Connector for SCEP connector and challenge lifecycle.
//!
//! Every case drives the service the way a client does — one HTTP request per call, through
//! the same `MultiServiceRouter` the emulator boots — and asserts only what the stored state
//! can justify: a connector read back through `GetConnector` exactly as it was created, a
//! challenge password that `GetChallengePassword` returns byte for byte, a page that resumes
//! where the previous one ended, and a 404 for a READ of an ARN nothing was ever created
//! under.
//!
//! The rows of each table differ along what the rule generalises over (connector type,
//! which optional members are present, the CA ARN, the client token's shape, how many
//! challenges exist, page size and the published `MaxResults` bounds), so no assertion is
//! pinned to one request.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{self, label, new_router, Answer};

/// A region that is NOT the emulator default, so an ARN built from the request's own SigV4
/// credential scope is distinguishable from a hardcoded one.
const REGION: &str = "eu-west-2";

/// The canonical emulator account id every ARN carries.
const ACCOUNT: &str = "000000000000";

/// `MaxResults`' published bounds on both listings, from their `@range(1, 1000)`.
const MAX_RESULTS_MIN: i64 = 1;
/// The upper published bound of `MaxResults`.
const MAX_RESULTS_MAX: i64 = 1000;

/// The two `ConnectorStatus` values this slice moves a connector through, named from the
/// service's own constants.
const STATUS_CREATING: &str =
    service_com_amazonaws_pcaconnectorscep_pcaconnectorscep::state::STATUS_CREATING;
/// The settled status the clock owes a new connector.
const STATUS_ACTIVE: &str =
    service_com_amazonaws_pcaconnectorscep_pcaconnectorscep::state::STATUS_ACTIVE;
/// The status a connector carries while `DeleteConnector`'s teardown runs.
const STATUS_DELETING: &str =
    service_com_amazonaws_pcaconnectorscep_pcaconnectorscep::state::STATUS_DELETING;

/// Private-CA ARNs matching `CertificateAuthorityArn`'s own pattern
/// (`^arn:aws(-[a-z]+)*:acm-pca:<region>:<account>:certificate-authority/<uuid>$`). Rows
/// differ in the CA they name, so nothing can pass by echoing one recorded value.
const CA_ARNS: &[&str] = &[
    "arn:aws:acm-pca:eu-west-2:000000000000:certificate-authority/11111111-2222-3333-4444-555555555555",
    "arn:aws:acm-pca:eu-west-2:000000000000:certificate-authority/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
    "arn:aws:acm-pca:eu-west-2:000000000000:certificate-authority/0123456f-89ab-cdef-0123-456789abcdef",
    "arn:aws:acm-pca:eu-west-2:000000000000:certificate-authority/deadbeef-0000-1111-2222-333344445555",
    "arn:aws:acm-pca:eu-west-2:000000000000:certificate-authority/99999999-8888-7777-6666-555544443333",
    "arn:aws:acm-pca:eu-west-2:000000000000:certificate-authority/abcdef01-2345-6789-abcd-ef0123456789",
];

async fn create_connector(router: &MultiServiceRouter, request: Value) -> Answer {
    harness::call(router, REGION, "POST", "/connectors", Some(request)).await
}

async fn get_connector(router: &MultiServiceRouter, arn: &str) -> Answer {
    harness::call(
        router,
        REGION,
        "GET",
        &format!("/connectors/{}", label(arn)),
        None,
    )
    .await
}

async fn list_connectors(router: &MultiServiceRouter, query: &str) -> Answer {
    harness::call(router, REGION, "GET", &format!("/connectors{query}"), None).await
}

async fn delete_connector(router: &MultiServiceRouter, arn: &str) -> Answer {
    harness::call(
        router,
        REGION,
        "DELETE",
        &format!("/connectors/{}", label(arn)),
        None,
    )
    .await
}

async fn create_challenge(router: &MultiServiceRouter, request: Value) -> Answer {
    harness::call(router, REGION, "POST", "/challenges", Some(request)).await
}

async fn get_challenge_password(router: &MultiServiceRouter, arn: &str) -> Answer {
    harness::call(
        router,
        REGION,
        "GET",
        &format!("/challengePasswords/{}", label(arn)),
        None,
    )
    .await
}

async fn delete_challenge(router: &MultiServiceRouter, arn: &str) -> Answer {
    harness::call(
        router,
        REGION,
        "DELETE",
        &format!("/challenges/{}", label(arn)),
        None,
    )
    .await
}

async fn list_challenge_metadata(
    router: &MultiServiceRouter,
    connector_arn: &str,
    extra: &str,
) -> Answer {
    harness::call(
        router,
        REGION,
        "GET",
        &format!(
            "/challengeMetadata?ConnectorArn={}{extra}",
            label(connector_arn)
        ),
        None,
    )
    .await
}

/// Create a connector and make the `ACTIVE` the clock owes it due, so it is usable.
///
/// The due instant is moved through the service's own state rather than slept out, so a case
/// that is not about the transition does not pay the service's `TRANSITION_SPAN` for it and
/// cannot race
/// it either.
async fn active_connector(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    ca_arn: &str,
) -> String {
    let created = create_connector(router, json!({ "CertificateAuthorityArn": ca_arn })).await;
    assert_eq!(
        created.status, 202,
        "case {ca_arn:?}: expected 202 from CreateConnector, got {} / {:?}",
        created.status, created.body
    );
    let arn = created.text("ConnectorArn");
    harness::release_pending(engine, &arn);
    arn
}

/// True when `value` has the UUID form every id in this service's ARNs, and its challenge
/// `Password`, carries: `[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}`.
fn is_uuid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && [8usize, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(expected, group)| {
                group.len() == *expected
                    && group.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
            })
}

/// The connector-ARN form `ConnectorArn`'s pattern requires, for the region and account this
/// request was signed for.
fn connector_arn_prefix() -> String {
    format!("arn:aws:pca-connector-scep:{REGION}:{ACCOUNT}:connector/")
}

/// Every `Arn` in a listing body, under `items`.
fn arns_of(answer: &Answer, items: &str) -> Vec<String> {
    answer.body[items]
        .as_array()
        .unwrap_or_else(|| panic!("expected a `{items}` array in {:?}", answer.body))
        .iter()
        .map(|entry| {
            entry["Arn"]
                .as_str()
                .unwrap_or_else(|| panic!("expected an `Arn` in {entry:?}"))
                .to_string()
        })
        .collect()
}

/// Walk every page of a listing, answering the ARNs in order of appearance.
async fn page_through_connectors(router: &MultiServiceRouter, max_results: i64) -> Vec<String> {
    let mut seen = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let query = match &token {
            Some(next) => format!("?MaxResults={max_results}&NextToken={}", label(next)),
            None => format!("?MaxResults={max_results}"),
        };
        let page = list_connectors(router, &query).await;
        assert_eq!(
            page.status, 200,
            "paging connectors at MaxResults={max_results}: expected 200, got {} / {:?}",
            page.status, page.body
        );
        seen.extend(arns_of(&page, "Connectors"));
        token = page.body["NextToken"].as_str().map(str::to_string);
        if token.is_none() {
            return seen;
        }
        assert!(
            seen.len() <= 64,
            "paging connectors at MaxResults={max_results}: the cursor never terminated, got {seen:?}"
        );
    }
}

/// Walk every page of one connector's challenge metadata.
async fn page_through_challenges(
    router: &MultiServiceRouter,
    connector_arn: &str,
    max_results: i64,
) -> Vec<String> {
    let mut seen = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let extra = match &token {
            Some(next) => format!("&MaxResults={max_results}&NextToken={}", label(next)),
            None => format!("&MaxResults={max_results}"),
        };
        let page = list_challenge_metadata(router, connector_arn, &extra).await;
        assert_eq!(
            page.status, 200,
            "paging challenges at MaxResults={max_results}: expected 200, got {} / {:?}",
            page.status, page.body
        );
        for entry in page.body["Challenges"]
            .as_array()
            .unwrap_or_else(|| panic!("expected a `Challenges` array in {:?}", page.body))
        {
            assert!(
                entry.get("Password").is_none(),
                "a ChallengeMetadataSummary must never carry a password, got {entry:?}"
            );
        }
        seen.extend(arns_of(&page, "Challenges"));
        token = page.body["NextToken"].as_str().map(str::to_string);
        if token.is_none() {
            return seen;
        }
        assert!(
            seen.len() <= 64,
            "paging challenges at MaxResults={max_results}: the cursor never terminated, got {seen:?}"
        );
    }
}

/// One `CreateConnector` request shape, and what the connector it creates must report.
#[derive(Debug)]
struct ConnectorCase {
    name: &'static str,
    ca_arn: &'static str,
    mobile_device_management: Option<Value>,
    vpc_endpoint_id: Option<&'static str>,
    tags: Option<Value>,
    expected_type: &'static str,
}

/// A create followed by a get must return exactly what was created: the `Type` the request
/// implies, the CA ARN it named, the MDM settings it carried, an `Endpoint` derived from the
/// connector's own id, and OIDC parameters only where the connector is an Intune one.
#[tokio::test]
async fn created_connector_is_read_back_from_state() {
    let cases = [
        ConnectorCase {
            name: "general purpose, nothing optional",
            ca_arn: CA_ARNS[0],
            mobile_device_management: None,
            vpc_endpoint_id: None,
            tags: None,
            expected_type: "GENERAL_PURPOSE",
        },
        ConnectorCase {
            name: "general purpose with a 13-character VPC endpoint id and tags",
            ca_arn: CA_ARNS[1],
            mobile_device_management: None,
            // `VpcEndpointId` is `@length(13, 22)` with pattern
            // `^vpce-[0-9a-f]{8}([0-9a-f]{9})?$`: this is the short form.
            vpc_endpoint_id: Some("vpce-0123abcd"),
            tags: Some(json!({ "team": "pki", "env": "test" })),
            expected_type: "GENERAL_PURPOSE",
        },
        ConnectorCase {
            name: "intune",
            ca_arn: CA_ARNS[2],
            mobile_device_management: Some(json!({
                "Intune": {
                    "AzureApplicationId": "01234567-89ab-cdef-0123-456789abcdef",
                    "Domain": "example.onmicrosoft.com",
                }
            })),
            vpc_endpoint_id: None,
            tags: None,
            expected_type: "INTUNE",
        },
        ConnectorCase {
            name: "intune with a 22-character VPC endpoint id",
            ca_arn: CA_ARNS[3],
            mobile_device_management: Some(json!({
                "Intune": {
                    "AzureApplicationId": "fedcba98-7654-3210-fedc-ba9876543210",
                    "Domain": "contoso.example.com",
                }
            })),
            // The long form of the same pattern.
            vpc_endpoint_id: Some("vpce-0123abcd456789abc"),
            tags: Some(json!({ "owner": "scep" })),
            expected_type: "INTUNE",
        },
    ];

    for case in &cases {
        let (router, engine) = new_router();
        let mut request = json!({ "CertificateAuthorityArn": case.ca_arn });
        if let Some(mdm) = &case.mobile_device_management {
            request["MobileDeviceManagement"] = mdm.clone();
        }
        if let Some(vpce) = case.vpc_endpoint_id {
            request["VpcEndpointId"] = json!(vpce);
        }
        if let Some(tags) = &case.tags {
            request["Tags"] = tags.clone();
        }

        let created = create_connector(&router, request).await;
        assert_eq!(
            created.status, 202,
            "case {:?} ({case:?}): expected 202 from CreateConnector, got {} / {:?}",
            case.name, created.status, created.body
        );
        let arn = created.text("ConnectorArn");
        let prefix = connector_arn_prefix();
        let id = arn.strip_prefix(&prefix).unwrap_or_else(|| {
            panic!("case {case:?}: expected a ConnectorArn starting {prefix:?}, got {arn:?}")
        });
        assert!(
            is_uuid(id),
            "case {case:?}: the connector id must be a uuid, got {id:?}"
        );

        // `CreateConnector` answers 202 and `ConnectorStatus` carries `CREATING`, so a read
        // that beats the span the clock owes sees the transient status. The span is held open
        // through the service's own state, so this is what the read observes every run rather
        // than a race the case has to win.
        harness::hold_pending(&engine, &arn);
        let creating = get_connector(&router, &arn).await;
        assert_eq!(
            creating.connector("Status"),
            &json!(STATUS_CREATING),
            "case {case:?}: a connector whose transition is not due yet must report CREATING, \
             got {:?}",
            creating.body
        );

        // And once the clock owes it, the same read reports the settled status.
        harness::release_pending(&engine, &arn);
        let read = get_connector(&router, &arn).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from GetConnector, got {} / {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.connector("Arn"),
            &json!(arn),
            "case {case:?}: GetConnector must report the ARN it was asked for, got {:?}",
            read.body
        );
        assert_eq!(
            read.connector("CertificateAuthorityArn"),
            &json!(case.ca_arn),
            "case {case:?}: the stored CA ARN must come back, got {:?}",
            read.body
        );
        assert_eq!(
            read.connector("Type"),
            &json!(case.expected_type),
            "case {case:?}: expected Type {:?}, got {:?}",
            case.expected_type,
            read.body
        );
        assert_eq!(
            read.connector("Status"),
            &json!(STATUS_ACTIVE),
            "case {case:?}: the connector must settle to ACTIVE, got {:?}",
            read.body
        );
        let endpoint = read.connector("Endpoint").as_str().unwrap_or_default();
        assert!(
            endpoint.starts_with("https://") && endpoint.contains(id),
            "case {case:?}: the Endpoint must be an HTTPS URL derived from the connector id \
             {id:?}, got {endpoint:?}"
        );
        for member in ["CreatedAt", "UpdatedAt"] {
            assert!(
                read.connector(member).is_number(),
                "case {case:?}: expected a timestamp `{member}`, got {:?}",
                read.body
            );
        }
        assert_eq!(
            read.connector("MobileDeviceManagement"),
            case.mobile_device_management.as_ref().unwrap_or(&Value::Null),
            "case {case:?}: MobileDeviceManagement must echo what was stored, got {:?}",
            read.body
        );
        // OIDC parameters are for Microsoft Intune, so only an Intune connector has them —
        // and all three are published by the service, not read back out of the request.
        let oidc = read.connector("OpenIdConfiguration").clone();
        if case.expected_type == "INTUNE" {
            let issuer = oidc["Issuer"].as_str().unwrap_or_default();
            assert!(
                issuer.starts_with("https://") && issuer != endpoint && issuer.contains(id),
                "case {case:?}: the OIDC issuer must be an HTTPS issuer URL of this service \
                 naming the connector, and not its SCEP endpoint {endpoint:?}, got {oidc:?}"
            );
            assert_ne!(
                oidc["Subject"],
                json!(arn),
                "case {case:?}: the OIDC subject must be minted inside the issuer's namespace \
                 rather than being the connector ARN, got {oidc:?}"
            );
            assert_eq!(
                oidc["Audience"],
                // The one audience Microsoft Entra workload identity federation accepts in
                // an exchanged token; it is the same for every connector.
                json!("api://AzureADTokenExchange"),
                "case {case:?}: the OIDC audience must be the Entra token-exchange audience, \
                 got {oidc:?}"
            );
            let intune = case.mobile_device_management.as_ref().expect("intune case")["Intune"]
                .clone();
            for supplied in ["AzureApplicationId", "Domain"] {
                let value = intune[supplied].as_str().unwrap_or_default();
                for member in ["Issuer", "Subject", "Audience"] {
                    let published = oidc[member].as_str().unwrap_or_default();
                    assert!(
                        !published.contains(value),
                        "case {case:?}: OIDC {member} must not echo the request's {supplied} \
                         {value:?}, got {published:?}"
                    );
                }
            }
        } else {
            assert_eq!(
                oidc,
                Value::Null,
                "case {case:?}: a general-purpose connector publishes no OIDC parameters, got \
                 {oidc:?}"
            );
        }

        // And the same record is what the listing reports.
        let listed = list_connectors(&router, "").await;
        assert_eq!(
            arns_of(&listed, "Connectors"),
            vec![arn.clone()],
            "case {case:?}: ListConnectors must report the connector that was created, got {:?}",
            listed.body
        );
    }
}

/// A repeat create carrying the same `ClientToken` is a retry, not a second resource: it
/// answers with the ARN the first call created and state still holds exactly one connector.
/// A different token creates a second one.
#[tokio::test]
async fn client_token_makes_create_idempotent() {
    // `ClientToken` is `@length(1, 64)` with pattern `^[!-~]+$` — any printable ASCII run.
    let tokens = [
        ("single character", "a"),
        ("punctuation only", "!~-_.+"),
        // The published upper bound of `ClientToken`'s `@length(1, 64)`.
        ("sixty-four characters", &"t0ken".repeat(13)[..64]),
        ("uuid shaped", "7f8e9d0c-1b2a-3948-5766-859493827160"),
    ];

    for (case, token) in tokens {
        let router = new_router().0;
        let request = json!({
            "CertificateAuthorityArn": CA_ARNS[0],
            "ClientToken": token,
        });

        let first = create_connector(&router, request.clone()).await;
        assert_eq!(
            first.status, 202,
            "case {case:?}: expected 202 from the first CreateConnector, got {} / {:?}",
            first.status, first.body
        );
        let second = create_connector(&router, request.clone()).await;
        assert_eq!(
            second.text("ConnectorArn"),
            first.text("ConnectorArn"),
            "case {case:?}: the same ClientToken must answer with the same ConnectorArn"
        );

        // A different token of the same length, so a row already at the modelled 64-byte
        // ceiling stays inside it.
        let other_token = if token.len() < 64 {
            format!("{token}~")
        } else {
            format!("~{}", &token[1..])
        };
        let other = create_connector(
            &router,
            json!({
                "CertificateAuthorityArn": CA_ARNS[1],
                "ClientToken": other_token,
            }),
        )
        .await;
        assert_ne!(
            other.text("ConnectorArn"),
            first.text("ConnectorArn"),
            "case {case:?}: a different ClientToken must create a different connector"
        );

        let listed = list_connectors(&router, "").await;
        let arns = arns_of(&listed, "Connectors");
        assert_eq!(
            arns.len(),
            2,
            "case {case:?}: expected exactly two connectors in state, got {arns:?}"
        );
    }

    // A token whose connector has since been deleted binds nothing: the retry creates a new
    // connector, and the ARN it answers with really resolves.
    let (router, engine) = new_router();
    let request = json!({
        "CertificateAuthorityArn": CA_ARNS[2],
        "ClientToken": "token-for-a-deleted-connector",
    });
    let first = create_connector(&router, request.clone()).await;
    let first_arn = first.text("ConnectorArn");
    harness::release_pending(&engine, &first_arn);
    let deleted = delete_connector(&router, &first_arn).await;
    assert_eq!(
        deleted.status, 202,
        "case \"token of a deleted connector\": expected 202 from DeleteConnector, got {} / {:?}",
        deleted.status, deleted.body
    );
    let retried = create_connector(&router, request).await;
    let retried_arn = retried.text("ConnectorArn");
    assert_ne!(
        retried_arn, first_arn,
        "case \"token of a deleted connector\": the token must not answer with an ARN that \
         holds no record"
    );
    let read = get_connector(&router, &retried_arn).await;
    assert_eq!(
        read.status, 200,
        "case \"token of a deleted connector\": the ARN the retry answered must resolve, got {} / {:?}",
        read.status, read.body
    );

    // Without a token every create is its own connector.
    let router = new_router().0;
    let mut minted = BTreeSet::new();
    for ca_arn in &CA_ARNS[..3] {
        let created = create_connector(&router, json!({ "CertificateAuthorityArn": ca_arn })).await;
        minted.insert(created.text("ConnectorArn"));
    }
    assert_eq!(
        minted.len(),
        3,
        "case \"no ClientToken\": each create must mint its own connector, got {minted:?}"
    );
}

/// A repeat create carrying the same `ClientToken` AND the same inputs is a retry, not a
/// second resource. A token reused with different inputs stands for a different request, so
/// it is refused rather than answered for.
#[tokio::test]
async fn client_token_reused_with_different_inputs_conflicts() {
    // Each row changes exactly ONE resource-defining input of the first call.
    let first_request = json!({
        "CertificateAuthorityArn": CA_ARNS[0],
        "VpcEndpointId": "vpce-0123abcd",
        "ClientToken": "shared-token",
    });
    let mismatches = [
        (
            "different CertificateAuthorityArn",
            json!({
                "CertificateAuthorityArn": CA_ARNS[1],
                "VpcEndpointId": "vpce-0123abcd",
                "ClientToken": "shared-token",
            }),
        ),
        (
            "different VpcEndpointId",
            json!({
                "CertificateAuthorityArn": CA_ARNS[0],
                "VpcEndpointId": "vpce-0123abcd456789abc",
                "ClientToken": "shared-token",
            }),
        ),
        (
            "VpcEndpointId dropped",
            json!({
                "CertificateAuthorityArn": CA_ARNS[0],
                "ClientToken": "shared-token",
            }),
        ),
        (
            "MobileDeviceManagement added",
            json!({
                "CertificateAuthorityArn": CA_ARNS[0],
                "VpcEndpointId": "vpce-0123abcd",
                "MobileDeviceManagement": {
                    "Intune": {
                        "AzureApplicationId": "01234567-89ab-cdef-0123-456789abcdef",
                        "Domain": "example.onmicrosoft.com",
                    }
                },
                "ClientToken": "shared-token",
            }),
        ),
    ];

    for (case, mismatch) in mismatches {
        let router = new_router().0;
        let first = create_connector(&router, first_request.clone()).await;
        let arn = first.text("ConnectorArn");

        let retried = create_connector(&router, mismatch).await;
        assert_eq!(
            (retried.status, retried.error_type.as_str()),
            (409, "ConflictException"),
            "case {case:?}: a token reused with different inputs must conflict, got {} / {:?}",
            retried.status,
            retried.body
        );

        // The identical retry still answers with the connector the token created.
        let same = create_connector(&router, first_request.clone()).await;
        assert_eq!(
            same.text("ConnectorArn"),
            arn,
            "case {case:?}: the identical retry must still answer with the same ConnectorArn"
        );
    }

    // The same rule on CreateChallenge, whose only resource-defining input is the connector.
    let (router, engine) = new_router();
    let first_connector = active_connector(&router, &engine, CA_ARNS[2]).await;
    let other_connector = active_connector(&router, &engine, CA_ARNS[3]).await;
    let created = create_challenge(
        &router,
        json!({ "ConnectorArn": first_connector.clone(), "ClientToken": "challenge-token" }),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "case \"CreateChallenge token reuse\": expected 202 from the first call, got {} / {:?}",
        created.status, created.body
    );
    let retried = create_challenge(
        &router,
        json!({ "ConnectorArn": other_connector, "ClientToken": "challenge-token" }),
    )
    .await;
    assert_eq!(
        (retried.status, retried.error_type.as_str()),
        (409, "ConflictException"),
        "case \"CreateChallenge token reuse\": a token reused for another connector must \
         conflict, got {} / {:?}",
        retried.status,
        retried.body
    );
    let same = create_challenge(
        &router,
        json!({ "ConnectorArn": first_connector, "ClientToken": "challenge-token" }),
    )
    .await;
    assert_eq!(
        same.challenge("Arn"),
        created.challenge("Arn"),
        "case \"CreateChallenge token reuse\": the identical retry must answer with the same \
         challenge"
    );
}

/// A challenge password round-trips: `GetChallengePassword` answers byte for byte what
/// `CreateChallenge` returned, the metadata listing names exactly the challenges of the
/// connector that was asked about, and no metadata response carries a password.
#[tokio::test]
async fn challenge_passwords_round_trip_and_stay_out_of_metadata() {
    // The row's count is what the rule generalises over: one challenge, a handful, and more
    // than one page's worth at the smallest page size.
    for count in [1usize, 2, 5] {
        let (router, engine) = new_router();
        let connector = active_connector(&router, &engine, CA_ARNS[0]).await;
        let other = active_connector(&router, &engine, CA_ARNS[1]).await;

        let mut created: Vec<(String, String)> = Vec::new();
        for index in 0..count {
            let answer =
                create_challenge(&router, json!({ "ConnectorArn": connector.clone() })).await;
            assert_eq!(
                answer.status, 202,
                "case {count} challenges, #{index}: expected 202 from CreateChallenge, got {} / {:?}",
                answer.status, answer.body
            );
            let arn = answer
                .challenge("Arn")
                .as_str()
                .unwrap_or_else(|| panic!("expected a challenge `Arn` in {:?}", answer.body))
                .to_string();
            let password = answer
                .challenge("Password")
                .as_str()
                .unwrap_or_else(|| panic!("expected a `Password` in {:?}", answer.body))
                .to_string();
            let expected_prefix = format!("{connector}/challenge/");
            let id = arn.strip_prefix(&expected_prefix).unwrap_or_else(|| {
                panic!(
                    "case {count} challenges, #{index}: expected an Arn starting \
                     {expected_prefix:?}, got {arn:?}"
                )
            });
            assert!(
                is_uuid(id),
                "case {count} challenges, #{index}: the challenge id must be a uuid, got {id:?}"
            );
            assert!(
                is_uuid(&password),
                "case {count} challenges, #{index}: the password must be in UUID format, got \
                 {password:?}"
            );
            assert_eq!(
                answer.challenge("ConnectorArn"),
                &json!(connector),
                "case {count} challenges, #{index}: the challenge must name its connector, got {:?}",
                answer.body
            );
            created.push((arn, password));
        }

        for (arn, password) in &created {
            let read = get_challenge_password(&router, arn).await;
            assert_eq!(
                read.status, 200,
                "case {count} challenges, {arn:?}: expected 200 from GetChallengePassword, got \
                 {} / {:?}",
                read.status, read.body
            );
            assert_eq!(
                read.text("Password"),
                *password,
                "case {count} challenges, {arn:?}: the stored password must come back unchanged"
            );
        }

        let listed = page_through_challenges(&router, &connector, MAX_RESULTS_MAX).await;
        let expected: BTreeSet<String> = created.iter().map(|(arn, _)| arn.clone()).collect();
        assert_eq!(
            listed.iter().cloned().collect::<BTreeSet<_>>(),
            expected,
            "case {count} challenges: ListChallengeMetadata must report exactly this \
             connector's challenges, got {listed:?}"
        );

        let elsewhere = page_through_challenges(&router, &other, MAX_RESULTS_MAX).await;
        assert!(
            elsewhere.is_empty(),
            "case {count} challenges: another connector must not list them, got {elsewhere:?}"
        );
    }
}

/// Paging never repeats or skips a record, whatever the page size, and the published
/// `MaxResults` bounds are accepted while a value one past either bound is refused.
#[tokio::test]
async fn listings_page_without_repeating_or_skipping() {
    for (total, page_size) in [(1usize, 1i64), (3, 1), (3, 2), (5, 2), (4, 4), (3, 1000)] {
        let (router, engine) = new_router();
        let mut connectors = BTreeSet::new();
        for index in 0..total {
            let created = create_connector(
                &router,
                json!({ "CertificateAuthorityArn": CA_ARNS[index % CA_ARNS.len()] }),
            )
            .await;
            connectors.insert(created.text("ConnectorArn"));
        }

        let paged = page_through_connectors(&router, page_size).await;
        assert_eq!(
            paged.len(),
            total,
            "case {total} connectors at MaxResults={page_size}: expected {total} records with no \
             duplicates, got {paged:?}"
        );
        assert_eq!(
            paged.iter().cloned().collect::<BTreeSet<_>>(),
            connectors,
            "case {total} connectors at MaxResults={page_size}: paging must visit every \
             connector exactly once, got {paged:?}"
        );

        // The same cursor rule over the other paginated listing.
        let connector = connectors
            .iter()
            .next_back()
            .expect("at least one connector")
            .clone();
        harness::release_pending(&engine, &connector);
        let mut challenges = BTreeSet::new();
        for _ in 0..total {
            let answer =
                create_challenge(&router, json!({ "ConnectorArn": connector.clone() })).await;
            challenges.insert(
                answer
                    .challenge("Arn")
                    .as_str()
                    .unwrap_or_else(|| panic!("expected a challenge `Arn` in {:?}", answer.body))
                    .to_string(),
            );
        }
        let paged = page_through_challenges(&router, &connector, page_size).await;
        assert_eq!(
            paged.len(),
            total,
            "case {total} challenges at MaxResults={page_size}: expected {total} records with no \
             duplicates, got {paged:?}"
        );
        assert_eq!(
            paged.iter().cloned().collect::<BTreeSet<_>>(),
            challenges,
            "case {total} challenges at MaxResults={page_size}: paging must visit every \
             challenge exactly once, got {paged:?}"
        );
    }

    // The published bounds themselves, and one past each: `MaxResults` is `@range(1, 1000)`.
    let (router, engine) = new_router();
    let connector = active_connector(&router, &engine, CA_ARNS[0]).await;
    for (case, max_results, expect_ok) in [
        ("lower bound", MAX_RESULTS_MIN, true),
        ("upper bound", MAX_RESULTS_MAX, true),
        ("one below the lower bound", MAX_RESULTS_MIN - 1, false),
        ("one past the upper bound", MAX_RESULTS_MAX + 1, false),
    ] {
        for (listing, answer) in [
            (
                "ListConnectors",
                list_connectors(&router, &format!("?MaxResults={max_results}")).await,
            ),
            (
                "ListChallengeMetadata",
                list_challenge_metadata(&router, &connector, &format!("&MaxResults={max_results}"))
                    .await,
            ),
        ] {
            if expect_ok {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?} on {listing}: MaxResults={max_results} is inside the modelled \
                     range, expected 200, got {} / {:?}",
                    answer.status, answer.body
                );
            } else {
                assert_eq!(
                    answer.status, 400,
                    "case {case:?} on {listing}: MaxResults={max_results} is outside the modelled \
                     range, expected 400, got {} / {:?}",
                    answer.status, answer.body
                );
            }
        }
    }
}

/// A `NextToken` this service never minted names no position, so it is refused rather than
/// silently restarting the listing.
#[tokio::test]
async fn unminted_pagination_token_is_refused() {
    let (router, engine) = new_router();
    let connector = active_connector(&router, &engine, CA_ARNS[0]).await;

    // Each row matches `NextToken`'s own pattern (base64url with padding), so the boundary
    // validator admits it and the handler is what refuses it.
    for (case, token) in [
        ("base64url of unrelated text", "bm90LWEtY3Vyc29yLWF0LWFsbA=="),
        ("base64url of the text \"foo\"", "Zm9v"),
        ("padded four-character group", "YWJjZA=="),
    ] {
        for (listing, answer) in [
            (
                "ListConnectors",
                list_connectors(&router, &format!("?NextToken={}", label(token))).await,
            ),
            (
                "ListChallengeMetadata",
                list_challenge_metadata(
                    &router,
                    &connector,
                    &format!("&NextToken={}", label(token)),
                )
                .await,
            ),
        ] {
            assert_eq!(
                answer.status, 400,
                "case {case:?} on {listing}: expected 400 for a token this service never \
                 minted, got {} / {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_type, "ValidationException",
                "case {case:?} on {listing}: expected ValidationException, got {:?}",
                answer.error_type
            );
        }
    }
}

/// A `NextToken` is scoped to the listing that minted it: the connector listing's cursor
/// names no position in a challenge listing, and one connector's challenge cursor names none
/// in another connector's, so each is refused rather than silently skipping entries.
#[tokio::test]
async fn pagination_token_of_another_listing_is_refused() {
    let (router, engine) = new_router();
    // Two connectors, each with two challenges, so both listings really do mint a cursor.
    let first = active_connector(&router, &engine, CA_ARNS[0]).await;
    let second = active_connector(&router, &engine, CA_ARNS[1]).await;
    for connector in [&first, &second] {
        for _ in 0..2 {
            let answer = create_challenge(&router, json!({ "ConnectorArn": connector })).await;
            assert_eq!(
                answer.status, 202,
                "seeding {connector:?}: expected 202 from CreateChallenge, got {} / {:?}",
                answer.status, answer.body
            );
        }
    }

    let connectors_page = list_connectors(&router, "?MaxResults=1").await;
    let connectors_token = connectors_page.body["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a NextToken in {:?}", connectors_page.body))
        .to_string();
    let challenges_page = list_challenge_metadata(&router, &first, "&MaxResults=1").await;
    let challenges_token = challenges_page.body["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a NextToken in {:?}", challenges_page.body))
        .to_string();

    for (case, answer) in [
        (
            "ListConnectors' cursor fed to ListChallengeMetadata",
            list_challenge_metadata(
                &router,
                &first,
                &format!("&NextToken={}", label(&connectors_token)),
            )
            .await,
        ),
        (
            "one connector's challenge cursor fed to another connector's listing",
            list_challenge_metadata(
                &router,
                &second,
                &format!("&NextToken={}", label(&challenges_token)),
            )
            .await,
        ),
        (
            "ListChallengeMetadata's cursor fed to ListConnectors",
            list_connectors(
                &router,
                &format!("?NextToken={}", label(&challenges_token)),
            )
            .await,
        ),
    ] {
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (400, "ValidationException"),
            "case {case:?}: a cursor of another listing must be refused, got {} / {:?}",
            answer.status,
            answer.body
        );
    }

    // And each token still resumes the listing that minted it.
    let resumed = list_challenge_metadata(
        &router,
        &first,
        &format!("&NextToken={}", label(&challenges_token)),
    )
    .await;
    assert_eq!(
        resumed.status, 200,
        "case \"the cursor's own listing\": expected 200, got {} / {:?}",
        resumed.status, resumed.body
    );
    let resumed = list_connectors(
        &router,
        &format!("?NextToken={}", label(&connectors_token)),
    )
    .await;
    assert_eq!(
        resumed.status, 200,
        "case \"the cursor's own listing\": expected 200, got {} / {:?}",
        resumed.status, resumed.body
    );
}

/// A delete of a challenge removes the record, so the next reader cannot see it; a repeat
/// delete of the same ARN asks for the end state already reached, so it is accepted (202) with
/// nothing left to remove. Deleting a connector is accepted (202), takes its challenges with
/// it once the teardown is due, and a repeat after that is accepted too.
#[tokio::test]
async fn deletes_are_observable_and_idempotent() {
    for challenges_per_connector in [1usize, 3] {
        let (router, engine) = new_router();
        let connector = active_connector(&router, &engine, CA_ARNS[0]).await;
        let mut arns = Vec::new();
        for _ in 0..challenges_per_connector {
            let answer =
                create_challenge(&router, json!({ "ConnectorArn": connector.clone() })).await;
            arns.push(
                answer
                    .challenge("Arn")
                    .as_str()
                    .unwrap_or_else(|| panic!("expected a challenge `Arn` in {:?}", answer.body))
                    .to_string(),
            );
        }

        // Delete the first challenge: it stops being readable and stops being listed.
        let doomed = arns[0].clone();
        let deleted = delete_challenge(&router, &doomed).await;
        assert_eq!(
            deleted.status, 202,
            "case {challenges_per_connector} challenges: expected 202 from DeleteChallenge, got \
             {} / {:?}",
            deleted.status, deleted.body
        );
        let read = get_challenge_password(&router, &doomed).await;
        assert_eq!(
            (read.status, read.error_type.as_str()),
            (404, "ResourceNotFoundException"),
            "case {challenges_per_connector} challenges: a deleted challenge must answer 404, \
             got {} / {:?}",
            read.status,
            read.body
        );
        let listed = page_through_challenges(&router, &connector, MAX_RESULTS_MAX).await;
        assert!(
            !listed.contains(&doomed),
            "case {challenges_per_connector} challenges: a deleted challenge must not be listed, \
             got {listed:?}"
        );
        let again = delete_challenge(&router, &doomed).await;
        assert_eq!(
            (again.status, again.error_type.as_str()),
            (202, ""),
            "case {challenges_per_connector} challenges: a repeated DeleteChallenge asks for the \
             end state already reached, so it must answer 202, got {} / {:?}",
            again.status,
            again.body
        );
        let still_gone = get_challenge_password(&router, &doomed).await;
        assert_eq!(
            still_gone.status, 404,
            "case {challenges_per_connector} challenges: the repeat must not resurrect the \
             challenge, got {} / {:?}",
            still_gone.status, still_gone.body
        );

        // Delete the connector: it reports DELETING while the teardown runs, then stops
        // being readable and listed, and its remaining challenges go with it.
        let deleted = delete_connector(&router, &connector).await;
        assert_eq!(
            deleted.status, 202,
            "case {challenges_per_connector} challenges: expected 202 from DeleteConnector, got \
             {} / {:?}",
            deleted.status, deleted.body
        );
        let tearing_down = get_connector(&router, &connector).await;
        assert_eq!(
            tearing_down.connector("Status"),
            &json!(STATUS_DELETING),
            "case {challenges_per_connector} challenges: a connector under teardown must report \
             {STATUS_DELETING}, got {:?}",
            tearing_down.body
        );

        // Let the clock make the teardown due; the record goes with it.
        harness::release_pending(&engine, &connector);
        let read = get_connector(&router, &connector).await;
        assert_eq!(
            (read.status, read.error_type.as_str()),
            (404, "ResourceNotFoundException"),
            "case {challenges_per_connector} challenges: a deleted connector must answer 404, \
             got {} / {:?}",
            read.status,
            read.body
        );
        let listed = list_connectors(&router, "").await;
        assert!(
            arns_of(&listed, "Connectors").is_empty(),
            "case {challenges_per_connector} challenges: a deleted connector must not be listed, \
             got {:?}",
            listed.body
        );
        for arn in &arns[1..] {
            let read = get_challenge_password(&router, arn).await;
            assert_eq!(
                (read.status, read.error_type.as_str()),
                (404, "ResourceNotFoundException"),
                "case {challenges_per_connector} challenges: deleting the connector must drop \
                 {arn:?}, got {} / {:?}",
                read.status,
                read.body
            );
        }
        let again = delete_connector(&router, &connector).await;
        assert_eq!(
            (again.status, again.error_type.as_str()),
            (202, ""),
            "case {challenges_per_connector} challenges: a repeated DeleteConnector asks for the \
             end state already reached, so it must answer 202, got {} / {:?}",
            again.status,
            again.body
        );
        let still_gone = get_connector(&router, &connector).await;
        assert_eq!(
            still_gone.status, 404,
            "case {challenges_per_connector} challenges: the repeat must not resurrect the \
             connector, got {} / {:?}",
            still_gone.status, still_gone.body
        );
    }
}

/// Every operation that READS or acts on a named resource answers
/// `ResourceNotFoundException` for a well-formed ARN nothing was created under — never a 200
/// with an empty body. The two deletes are not in the table: their whole output is the
/// modelled 202 "accepted", and an ARN that holds no record is the end state they accept
/// (see `connector_delete_not_found` and `challenge_deletion`).
#[tokio::test]
async fn unknown_arns_answer_resource_not_found() {
    let router = new_router().0;
    // Each row is a well-formed ARN of the modelled shape that names no stored record: the
    // rule generalises over the id, not over one recorded value.
    let unknown_connectors = [
        format!("{}11111111-1111-1111-1111-111111111111", connector_arn_prefix()),
        format!("{}ffffffff-eeee-dddd-cccc-bbbbbbbbbbbb", connector_arn_prefix()),
    ];

    for connector in &unknown_connectors {
        let challenge = format!("{connector}/challenge/22222222-3333-4444-5555-666666666666");
        for (case, answer) in [
            ("GetConnector", get_connector(&router, connector).await),
            (
                "CreateChallenge",
                create_challenge(&router, json!({ "ConnectorArn": connector })).await,
            ),
            (
                "ListChallengeMetadata",
                list_challenge_metadata(&router, connector, "").await,
            ),
            (
                "GetChallengePassword",
                get_challenge_password(&router, &challenge).await,
            ),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {case} on {connector:?}: expected 404, got {} / {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_type, "ResourceNotFoundException",
                "case {case} on {connector:?}: expected ResourceNotFoundException, got {:?}",
                answer.error_type
            );
        }
    }
}

/// `CreateChallenge` is "for general-purpose connectors" and needs a connector that is
/// serving, so it is refused — with the reason the model carries for each — on an Intune
/// connector and on one the clock still owes its `ACTIVE`.
#[tokio::test]
async fn challenge_creation_is_refused_outside_an_active_general_purpose_connector() {
    for (case, request, expected_reason) in [
        (
            "intune connector",
            json!({
                "CertificateAuthorityArn": CA_ARNS[4],
                "MobileDeviceManagement": {
                    "Intune": {
                        "AzureApplicationId": "01234567-89ab-cdef-0123-456789abcdef",
                        "Domain": "example.onmicrosoft.com",
                    }
                },
            }),
            "INVALID_CONNECTOR_TYPE",
        ),
        (
            "connector still creating",
            json!({ "CertificateAuthorityArn": CA_ARNS[5] }),
            "INVALID_STATE",
        ),
    ] {
        let (router, engine) = new_router();
        let created = create_connector(&router, request).await;
        let arn = created.text("ConnectorArn");
        if expected_reason == "INVALID_CONNECTOR_TYPE" {
            // An Intune connector is refused for its type, not for its status, so settle it
            // first: the refusal under test must be the type one.
            harness::release_pending(&engine, &arn);
        } else {
            // Hold the transition the clock owes it, so the CREATING branch is the one
            // exercised rather than one the test had to win a race for.
            harness::hold_pending(&engine, &arn);
        }

        let reported = get_connector(&router, &arn).await;
        let expected_status = if expected_reason == "INVALID_CONNECTOR_TYPE" {
            STATUS_ACTIVE
        } else {
            STATUS_CREATING
        };
        assert_eq!(
            reported.connector("Status"),
            &json!(expected_status),
            "case {case:?}: the connector must report {expected_status} for this case, got {:?}",
            reported.body
        );

        let answer = create_challenge(&router, json!({ "ConnectorArn": arn.clone() })).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400 from CreateChallenge, got {} / {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_type, "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.error_type
        );
        assert_eq!(
            answer.body["Reason"],
            json!(expected_reason),
            "case {case:?}: expected Reason {expected_reason:?}, got {:?}",
            answer.body
        );
    }
}

/// `Tags` carries no modelled `@length`, so the per-resource tag limit is the handler's to
/// enforce: AWS's 50 tags per resource is accepted and 51 is refused, on both creates.
#[tokio::test]
async fn tag_count_limit_is_enforced_on_both_creates() {
    const MAX_TAGS: usize =
        service_com_amazonaws_pcaconnectorscep_pcaconnectorscep::state::MAX_TAGS_PER_RESOURCE;

    fn tags(count: usize) -> Value {
        let mut map = serde_json::Map::new();
        for index in 0..count {
            map.insert(format!("key-{index}"), json!(format!("value-{index}")));
        }
        Value::Object(map)
    }

    for (case, count, expected_status) in [
        ("the published limit", MAX_TAGS, 202),
        ("one past the published limit", MAX_TAGS + 1, 400),
        ("well inside the limit", 3, 202),
    ] {
        let (router, engine) = new_router();
        let created = create_connector(
            &router,
            json!({ "CertificateAuthorityArn": CA_ARNS[0], "Tags": tags(count) }),
        )
        .await;
        assert_eq!(
            created.status, expected_status,
            "case {case:?} ({count} tags) on CreateConnector: expected {expected_status}, got {} / {:?}",
            created.status, created.body
        );

        // The same rule on the other create, against a connector that is already serving.
        let connector = active_connector(&router, &engine, CA_ARNS[1]).await;
        let challenge = create_challenge(
            &router,
            json!({ "ConnectorArn": connector, "Tags": tags(count) }),
        )
        .await;
        assert_eq!(
            challenge.status, expected_status,
            "case {case:?} ({count} tags) on CreateChallenge: expected {expected_status}, got {} / {:?}",
            challenge.status, challenge.body
        );
    }
}

/// `DeleteConnector` answers 202 with an empty body, so it only ACCEPTS the teardown: a
/// connector that is still being created is deleted rather than refused, and the delete is
/// observable as `DELETING` before the record disappears.
#[tokio::test]
async fn deleting_a_connector_mid_creation_is_accepted() {
    let (router, engine) = new_router();
    let created = create_connector(&router, json!({ "CertificateAuthorityArn": CA_ARNS[0] })).await;
    let arn = created.text("ConnectorArn");

    // Hold the transition the clock owes this connector, so the CREATING branch is the one
    // exercised rather than one the test had to win a race for.
    harness::hold_pending(&engine, &arn);
    let reported = get_connector(&router, &arn).await;
    assert_eq!(
        reported.connector("Status"),
        &json!(STATUS_CREATING),
        "case \"delete while CREATING\": the connector must report CREATING, got {:?}",
        reported.body
    );
    let answer = delete_connector(&router, &arn).await;
    assert_eq!(
        answer.status, 202,
        "case \"delete while CREATING\": expected 202, got {} / {:?}",
        answer.status, answer.body
    );
    // The delete was accepted, so the connector is tearing down and no longer waiting for the
    // ACTIVE the clock used to owe it.
    let tearing_down = get_connector(&router, &arn).await;
    assert_eq!(
        tearing_down.connector("Status"),
        &json!(STATUS_DELETING),
        "case \"delete while CREATING\": the accepted delete must move it to DELETING, got {:?}",
        tearing_down.body
    );

    // Once the teardown is due the record is gone, and a delete of that ARN asks for the end
    // state already reached, so it is accepted with nothing left to schedule.
    harness::release_pending(&engine, &arn);
    let gone = get_connector(&router, &arn).await;
    assert_eq!(
        gone.status, 404,
        "case \"teardown due\": the connector must be gone, got {} / {:?}",
        gone.status, gone.body
    );
    let answer = delete_connector(&router, &arn).await;
    assert_eq!(
        (answer.status, answer.error_type.as_str()),
        (202, ""),
        "case \"delete once gone\": a delete of an ARN holding no record must answer 202, got {} \
         / {:?}",
        answer.status,
        answer.body
    );
}
