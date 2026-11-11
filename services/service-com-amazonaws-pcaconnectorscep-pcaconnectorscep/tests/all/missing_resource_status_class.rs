// SPDX-License-Identifier: CC0-1.0

//! Which status class a call about a resource that is NOT there answers.
//!
//! Three rules, one per route family, each asserted over a table rather than over one
//! recorded request:
//!
//! 1. **The deletes are idempotent in their STATUS.** `DeleteConnector` and `DeleteChallenge`
//!    are modelled with output `Unit` and http code 202 (smithy_query
//!    operation=DeleteConnector,DeleteChallenge), so the whole answer is "accepted" with an
//!    empty body, and what is accepted is the END STATE the call asks for. An ARN that holds
//!    no record is therefore the same 202 with nothing left to schedule — never a refusal.
//! 2. **The tagging endpoints report absence as the tag subsystem does.** `/tags/{ResourceArn}`
//!    is served by the resource-tagging layer, which answers an ARN it resolves to no resource
//!    with its own error class: HTTP 400, error code `NotFoundException`, and a single
//!    lower-case `message` member carrying one fixed sentence that names nothing of the
//!    request — not the service's 404 `ResourceNotFoundException` with its `Message`,
//!    `ResourceId` and `ResourceType`.
//! 3. **A create resolves the resource it REFERENCES against the service that owns it.**
//!    `CreateConnector`'s `CertificateAuthorityArn` names a private CA that Amazon Web
//!    Services Private CA owns, and `CreateConnector` models `ResourceNotFoundException`
//!    (HTTP 404): a CA that service reports it does not hold is that error, naming the input
//!    member as `ResourceType` and the supplied ARN as `ResourceId`, and no connector is
//!    created. So this suite mounts Private CA beside this service on ONE engine — what the
//!    assembled emulator does — and creates the CAs its accepting rows need through that
//!    service's own API.
//!
//! Every case drives the emulator the way a client does: one HTTP request per call through a
//! `MultiServiceRouter`. The rows differ along what each rule generalises over — the resource
//! kind, how the ARN came to name nothing, which tagging operation is called, and the
//! partition, Region, account and id of the referenced CA.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_pcaconnectorscep_pcaconnectorscep as scep;

use crate::harness::{self, label, new_router, Answer};

/// The account id every generated emulator presents, which both services mint ARNs in.
const ACCOUNT: &str = "000000000000";

/// The Region these cases are signed for, deliberately not the emulator default.
const REGION: &str = "eu-west-2";

/// The name of the `CreateConnector` input member that references the private CA, which is
/// what its `ResourceNotFoundException` carries as `ResourceType` (smithy_query
/// operation=CreateConnector).
const MEMBER_CERTIFICATE_AUTHORITY_ARN: &str = "CertificateAuthorityArn";

/// The sentence `ResourceNotFoundException` carries, spelled here independently of the
/// service's own helper so a change to either side is a failure rather than a tautology.
fn expected_message(resource_type: &str, resource_id: &str) -> String {
    format!("{resource_type} was not found: {resource_id}.")
}

/// A private-CA ARN of exactly the form `CertificateAuthorityArn`'s own `@pattern` requires:
/// `^arn:aws(-[a-z]+)*:acm-pca:<region>:<12 digits>:certificate-authority/<uuid>$`.
fn ca_arn(partition: &str, region: &str, account: &str, id: &str) -> String {
    format!("arn:{partition}:acm-pca:{region}:{account}:certificate-authority/{id}")
}

/// A connector ARN of exactly the form `ConnectorArn`'s own pattern requires.
fn connector_arn(region: &str, connector_id: &str) -> String {
    format!("arn:aws:pca-connector-scep:{region}:{ACCOUNT}:connector/{connector_id}")
}

// ---------------------------------------------------------------------------
// Rule 1: the deletes are idempotent in their status.
// ---------------------------------------------------------------------------

/// Which delete a row drives, and the path it is addressed at.
#[derive(Debug, Clone, Copy)]
enum Delete {
    Connector,
    Challenge,
}

impl Delete {
    async fn call(self, router: &MultiServiceRouter, arn: &str) -> Answer {
        let collection = match self {
            Self::Connector => "connectors",
            Self::Challenge => "challenges",
        };
        harness::call(
            router,
            REGION,
            "DELETE",
            &format!("/{collection}/{}", label(arn)),
            None,
        )
        .await
    }

    /// The READ of the same resource, which is where "it is not there" shows up as the
    /// service's own 404: the delete's own answer is a 202 either way, so the record's
    /// absence has to be checked through a reader.
    async fn read(self, router: &MultiServiceRouter, arn: &str) -> Answer {
        let path = match self {
            Self::Connector => format!("/connectors/{}", label(arn)),
            Self::Challenge => format!("/challengePasswords/{}", label(arn)),
        };
        harness::call(router, REGION, "GET", &path, None).await
    }
}

/// Every well-formed ARN that holds no record, for both deletes, is the accepted 202 with an
/// empty body — whatever made it hold none.
#[tokio::test]
async fn a_delete_of_a_resource_that_is_not_there_is_accepted() {
    // The rows: which delete, and an ARN of its own resource kind that names nothing. They
    // differ in the resource kind, the Region, the connector id and the challenge id. The
    // cases where the record was there and went — a repeat delete, a completed teardown — are
    // the tables of `connector_delete_not_found` and `challenge_deletion`.
    let absent_connector_id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let rows: Vec<(&str, Delete, String)> = vec![
        (
            "a connector ARN nothing was created under",
            Delete::Connector,
            connector_arn(REGION, "11111111-1111-1111-1111-111111111111"),
        ),
        (
            "a connector ARN of another Region",
            Delete::Connector,
            connector_arn("ap-southeast-2", "0123456f-89ab-cdef-0123-456789abcdef"),
        ),
        (
            "a challenge ARN nothing was created under",
            Delete::Challenge,
            format!(
                "{}/challenge/22222222-3333-4444-5555-666666666666",
                connector_arn(REGION, "ffffffff-eeee-dddd-cccc-bbbbbbbbbbbb")
            ),
        ),
        (
            "a challenge ARN under another connector that was never created",
            Delete::Challenge,
            format!(
                "{}/challenge/99999999-8888-7777-6666-555544443333",
                connector_arn(REGION, absent_connector_id)
            ),
        ),
    ];

    for (case, operation, arn) in &rows {
        let (router, _engine) = new_router();
        let answer = operation.call(&router, arn).await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (202, ""),
            "case {case:?} on {operation:?}: expected the modelled 202 with no error code for \
             {arn:?}, got {} / {:?}",
            answer.status,
            answer.body
        );
        assert!(
            answer.raw.is_empty() || answer.body == json!({}),
            "case {case:?} on {operation:?}: expected an empty body, got {:?}",
            String::from_utf8_lossy(&answer.raw)
        );
        // The accepted no-op created nothing a later reader could see.
        let read = operation.read(&router, arn).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: the accepted delete must leave the ARN naming nothing, got {} / {:?}",
            read.status, read.body
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the tagging endpoints answer the tag subsystem's own refusal.
// ---------------------------------------------------------------------------

/// Which tagging operation a row drives. All three share the one `/tags/{ResourceArn}` route.
#[derive(Debug, Clone, Copy)]
enum Tagging {
    List,
    Tag,
    Untag,
}

impl Tagging {
    async fn call(self, router: &MultiServiceRouter, arn: &str) -> Answer {
        let encoded = label(arn);
        match self {
            Self::List => harness::call(router, REGION, "GET", &format!("/tags/{encoded}"), None).await,
            Self::Tag => {
                harness::call(
                    router,
                    REGION,
                    "POST",
                    &format!("/tags/{encoded}"),
                    Some(json!({ "Tags": { "owner": "platform" } })),
                )
                .await
            }
            Self::Untag => {
                harness::call(
                    router,
                    REGION,
                    "DELETE",
                    &format!("/tags/{encoded}?tagKeys=owner"),
                    None,
                )
                .await
            }
        }
    }
}

/// Every tagging operation, on every kind of ARN that resolves to no resource here, answers
/// the tagging layer's own 400 `NotFoundException` with its one fixed lower-case `message`.
#[tokio::test]
async fn a_tagging_call_about_a_resource_that_is_not_there_is_the_tagging_layers_refusal() {
    let (router, engine) = new_router();

    // A connector and a challenge that DO exist, so no row below is answered out of an empty
    // store — and a torn-down pair, so a row names something this service once held.
    let live = create_connector(
        &router,
        json!({ "CertificateAuthorityArn": ca_arn("aws", REGION, ACCOUNT, "11111111-2222-3333-4444-555555555555") }),
    )
    .await
    .text("ConnectorArn");
    harness::release_pending(&engine, &live);
    let live_challenge = create_challenge(&router, &live).await;

    let doomed = create_connector(
        &router,
        json!({ "CertificateAuthorityArn": ca_arn("aws", REGION, ACCOUNT, "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee") }),
    )
    .await
    .text("ConnectorArn");
    harness::release_pending(&engine, &doomed);
    let doomed_challenge = create_challenge(&router, &doomed).await;
    assert_eq!(
        Delete::Connector.call(&router, &doomed).await.status,
        202,
        "the teardown of the connector a row names must be accepted"
    );
    harness::release_pending(&engine, &doomed);

    // The rows: every way an ARN that COULD name a resource of this service names none.
    let rows: Vec<(&str, String)> = vec![
        (
            "a connector ARN nothing was created under",
            connector_arn(REGION, "11111111-1111-1111-1111-111111111111"),
        ),
        (
            "a challenge ARN nothing was created under",
            format!(
                "{}/challenge/22222222-3333-4444-5555-666666666666",
                connector_arn(REGION, "ffffffff-eeee-dddd-cccc-bbbbbbbbbbbb")
            ),
        ),
        (
            "a challenge id that names nothing under a live connector",
            format!("{live}/challenge/33333333-4444-5555-6666-777777777777"),
        ),
        ("a connector whose teardown has come due", doomed.clone()),
        (
            "a challenge its connector's teardown took with it",
            doomed_challenge.clone(),
        ),
        (
            "a connector ARN of another partition",
            format!(
                "arn:aws-us-gov:pca-connector-scep:us-gov-west-1:{ACCOUNT}:connector/\
                 0123456f-89ab-cdef-0123-456789abcdef"
            ),
        ),
        (
            "an ARN of another service entirely",
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:bucket/not-a-scep-resource"),
        ),
    ];

    for (case, arn) in &rows {
        for operation in [Tagging::List, Tagging::Tag, Tagging::Untag] {
            let answer = operation.call(&router, arn).await;
            harness::assert_tagging_not_found(&answer, &format!("{case} on {operation:?}"));
        }
    }

    // None of those refusals touched what does exist: both live resources still report their
    // own tags through the same route.
    for (what, arn) in [("connector", &live), ("challenge", &live_challenge)] {
        let listed = Tagging::List.call(&router, arn).await;
        assert_eq!(
            listed.status, 200,
            "the live {what} must still be served by the tagging route, got {} / {:?}",
            listed.status, listed.body
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: CreateConnector resolves the private CA it references.
// ---------------------------------------------------------------------------

/// An emulator holding this service AND Amazon Web Services Private CA over one shared
/// engine, which is what the assembled emulator mounts.
fn emulator_with_private_ca() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signature verification is the emulator's own concern and is exercised elsewhere; these
    // cases are about these services' behaviour.
    router.set_request_authentication(false);
    router.register_service(scep::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_acmpca_acmprivateca::EmulatedService::new(Arc::clone(&engine)),
    );
    (router, engine)
}

/// One private CA created through Private CA's OWN `CreateCertificateAuthority`, answering the
/// ARN that service minted for it — so what `CreateConnector` resolves is a CA the owning
/// service holds, not one this suite made up.
async fn private_ca(router: &MultiServiceRouter, region: &str, common_name: &str) -> String {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://acm-pca.{region}.amazonaws.com/"))
        .header("host", format!("acm-pca.{region}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", "ACMPrivateCA.CreateCertificateAuthority")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{region}/acm-pca/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&json!({
                "CertificateAuthorityConfiguration": {
                    "KeyAlgorithm": "RSA_2048",
                    "SigningAlgorithm": "SHA256WITHRSA",
                    "Subject": {"CommonName": common_name},
                },
                "CertificateAuthorityType": "ROOT",
            }))
            .expect("serialize body"),
        ))
        .expect("build request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body: Value = serde_json::from_slice(&response.into_body()).unwrap_or(Value::Null);
    assert_eq!(
        status, 200,
        "CreateCertificateAuthority must answer 200, got {status} / {body:?}"
    );
    body["CertificateAuthorityArn"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a CertificateAuthorityArn in {body:?}"))
        .to_string()
}

async fn create_connector(router: &MultiServiceRouter, request: Value) -> Answer {
    let answer = harness::call(router, REGION, "POST", "/connectors", Some(request)).await;
    assert_eq!(
        answer.status, 202,
        "expected 202 from CreateConnector, got {} / {:?}",
        answer.status, answer.body
    );
    answer
}

async fn create_challenge(router: &MultiServiceRouter, connector_arn: &str) -> String {
    let answer = harness::call(
        router,
        REGION,
        "POST",
        "/challenges",
        Some(json!({ "ConnectorArn": connector_arn })),
    )
    .await;
    assert_eq!(
        answer.status, 202,
        "expected 202 from CreateChallenge, got {} / {:?}",
        answer.status, answer.body
    );
    answer.challenge("Arn")
        .as_str()
        .unwrap_or_else(|| panic!("expected a challenge Arn in {:?}", answer.body))
        .to_string()
}

/// `CreateConnector` on a `CertificateAuthorityArn` Private CA holds no certificate authority
/// under is the modelled 404, naming the input member and the ARN it was handed — and admits
/// no connector.
#[tokio::test]
async fn create_connector_refuses_a_private_ca_that_does_not_exist() {
    // The rows: well-formed CA ARNs that Private CA holds nothing under. They differ along
    // the partition, the Region, the account and the CA id, so no assertion can pass by
    // echoing one value.
    let rows: Vec<(&str, String)> = vec![
        (
            "a CA of this Region and account that was never created",
            ca_arn("aws", REGION, ACCOUNT, "11111111-2222-3333-4444-555555555555"),
        ),
        (
            "another CA id of this Region and account",
            ca_arn("aws", REGION, ACCOUNT, "0123456f-89ab-cdef-0123-456789abcdef"),
        ),
        (
            "a CA of another Region",
            ca_arn("aws", "ap-southeast-2", ACCOUNT, "deadbeef-0000-1111-2222-333344445555"),
        ),
        (
            "a CA of another account",
            ca_arn("aws", REGION, "123456789012", "99999999-8888-7777-6666-555544443333"),
        ),
        (
            "a CA of another partition",
            ca_arn(
                "aws-us-gov",
                "us-gov-west-1",
                ACCOUNT,
                "abcdef01-2345-6789-abcd-ef0123456789",
            ),
        ),
    ];

    for (case, arn) in &rows {
        let (router, _engine) = emulator_with_private_ca();
        let answer = harness::call(
            &router,
            REGION,
            "POST",
            "/connectors",
            Some(json!({ "CertificateAuthorityArn": arn })),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected 404 ResourceNotFoundException for {arn:?}, got {} / {:?}",
            answer.status,
            answer.body
        );
        assert_eq!(
            answer.message(),
            expected_message(MEMBER_CERTIFICATE_AUTHORITY_ARN, arn),
            "case {case:?}: expected AWS's not-found sentence, got {:?}",
            answer.message()
        );
        assert_eq!(
            (
                answer.body["ResourceId"].as_str(),
                answer.body["ResourceType"].as_str()
            ),
            (Some(arn.as_str()), Some(MEMBER_CERTIFICATE_AUTHORITY_ARN)),
            "case {case:?}: the refusal must name the supplied ARN and the input member, got \
             {:?}",
            answer.body
        );

        // Nothing was admitted: the refusal left no connector behind for any later call to
        // find.
        let listed = harness::call(&router, REGION, "GET", "/connectors", None).await;
        assert_eq!(
            listed.count("Connectors"),
            0,
            "case {case:?}: the refused create must register no connector, got {:?}",
            listed.body
        );
    }
}

/// The other side of the same rule: a `CertificateAuthorityArn` Private CA DOES hold is
/// admitted, and the connector reads back carrying exactly that ARN.
#[tokio::test]
async fn create_connector_admits_a_private_ca_that_exists() {
    // Rows differ in the Region the CA and the connector live in and in the CA's subject, so
    // each row resolves a different ARN that service minted.
    for (case, region, common_name) in [
        ("a CA of the Region the create is signed for", REGION, "scep-ca-one"),
        ("another Region, another subject", "us-west-2", "scep-ca-two"),
    ] {
        let (router, engine) = emulator_with_private_ca();
        let arn = private_ca(&router, region, common_name).await;

        let created = harness::call(
            &router,
            region,
            "POST",
            "/connectors",
            Some(json!({ "CertificateAuthorityArn": arn })),
        )
        .await;
        assert_eq!(
            (created.status, created.error_type.as_str()),
            (202, ""),
            "case {case:?}: a CA Private CA holds must be admitted, got {} / {:?}",
            created.status,
            created.body
        );
        let connector = created.text("ConnectorArn");
        harness::release_pending(&engine, &connector);

        let read = harness::call(
            &router,
            region,
            "GET",
            &format!("/connectors/{}", label(&connector)),
            None,
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the admitted connector must read back, got {} / {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.connector("CertificateAuthorityArn"),
            &json!(arn),
            "case {case:?}: the connector must carry the CA ARN it was created from, got {:?}",
            read.body
        );
    }
}
