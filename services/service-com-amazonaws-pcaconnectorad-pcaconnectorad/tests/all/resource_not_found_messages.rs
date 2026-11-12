// SPDX-License-Identifier: CC0-1.0

//! How this service words the refusal of a request naming a resource it does not hold.
//!
//! ONE sentence for every resource type and every operation — the type, the identifier
//! the request supplied, then a full stop (`crate::state::not_found_message`) — so a
//! caller reads the same refusal whichever resource went missing, and the identifier it
//! reads back is the one it sent. Each row drives the real wire path through
//! [`super::harness`] with an identifier its member's `@pattern` admits, so what refuses
//! it is the handler's lookup and not the boundary validator.
//!
//! The rows span the three resources this service holds (connector, template, directory
//! registration), a read and a create, the prerequisite a create resolves before it
//! mints anything, and identifiers that are absent in three different ways: never
//! created in this Region, named in another Region, and named in another partition.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{call, encode_label, router, ACCOUNT, REGION};

/// One refusal: a request addressing a resource this store does not hold.
struct MissingCase {
    /// Why this row is here: the operation and the way the resource is absent.
    case: String,
    method: &'static str,
    target: String,
    body: Option<Value>,
    /// The `ResourceType` the refusal names, as the modelled shape spells it.
    resource_type: &'static str,
    /// The identifier the request supplied, which the refusal echoes.
    identifier: String,
}

/// A connector ARN of this emulator's Region and account that no call ever minted.
fn connector_arn(uuid: &str) -> String {
    format!("arn:aws:pca-connector-ad:{REGION}:{ACCOUNT}:connector/{uuid}")
}

/// A template ARN under a connector that no call ever minted.
fn template_arn(connector_uuid: &str, template_uuid: &str) -> String {
    format!("{}/template/{template_uuid}", connector_arn(connector_uuid))
}

/// The body a `CreateTemplate` carries: the parent connector, a name the modelled
/// `TemplateName` pattern admits and the smallest `TemplateV2` definition the union
/// accepts (`smithy_query(query="operation",operation="CreateTemplate")` — the
/// connector is a BODY member of this operation, not a label).
fn create_template_body(connector_arn: &str) -> Value {
    json!({
        "ConnectorArn": connector_arn,
        "Name": "WebServerTemplate",
        "Definition": { "TemplateV2": {
            "CertificateValidity": {
                "ValidityPeriod": { "PeriodType": "YEARS", "Period": 1 },
                "RenewalPeriod": { "PeriodType": "WEEKS", "Period": 6 },
            },
            "PrivateKeyAttributes": { "MinimalKeyLength": 2048, "KeySpec": "KEY_EXCHANGE" },
            "PrivateKeyFlags": { "ClientVersion": "WINDOWS_SERVER_2016" },
            "EnrollmentFlags": { "NoSecurityExtension": true },
            "SubjectNameFlags": { "SanRequireDirectoryGuid": true },
            "GeneralFlags": { "MachineType": true },
            "Extensions": {
                "KeyUsage": { "UsageFlags": { "KeyEncipherment": true, "DigitalSignature": true } }
            },
        }},
    })
}

/// The body a `CreateConnector` carries: the directory whose registration is the
/// create's prerequisite, a pattern-valid CA ARN and the smallest
/// `VpcInformation` the shape accepts
/// (`smithy_query(query="operation",operation="CreateConnector")`).
fn create_connector_body(directory_id: &str) -> Value {
    json!({
        "DirectoryId": directory_id,
        "CertificateAuthorityArn": format!(
            "arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate-authority/\
             22222222-3333-4444-5555-666666666666"
        ),
        "VpcInformation": { "SecurityGroupIds": ["sg-0123456789abcdef0"] },
    })
}

fn missing_cases() -> Vec<MissingCase> {
    let unknown_connector = connector_arn("cccccccc-dddd-eeee-ffff-000000000000");
    let other_region_connector =
        format!("arn:aws:pca-connector-ad:eu-west-1:{ACCOUNT}:connector/11111111-2222-3333-4444-555555555555");
    let other_partition_template = format!(
        "arn:aws-us-gov:pca-connector-ad:{REGION}:{ACCOUNT}:connector/\
         11111111-2222-3333-4444-555555555555/template/66666666-7777-8888-9999-aaaaaaaaaaaa"
    );
    let unknown_template =
        template_arn("cccccccc-dddd-eeee-ffff-000000000000", "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee");
    let unknown_registration =
        format!("arn:aws:pca-connector-ad:{REGION}:{ACCOUNT}:directory-registration/d-5555555555");
    let unregistered_directory = "d-7777777777".to_string();
    vec![
        MissingCase {
            case: "GetConnector, a connector never created".to_string(),
            method: "GET",
            target: format!("/connectors/{}", encode_label(&unknown_connector)),
            body: None,
            resource_type: "Connector",
            identifier: unknown_connector.clone(),
        },
        MissingCase {
            case: "GetConnector, a connector of another Region".to_string(),
            method: "GET",
            target: format!("/connectors/{}", encode_label(&other_region_connector)),
            body: None,
            resource_type: "Connector",
            identifier: other_region_connector,
        },
        MissingCase {
            case: "CreateTemplate, a parent connector never created".to_string(),
            method: "POST",
            target: "/templates".to_string(),
            body: Some(create_template_body(&unknown_connector)),
            resource_type: "Connector",
            identifier: unknown_connector,
        },
        MissingCase {
            case: "GetTemplate, a template never created".to_string(),
            method: "GET",
            target: format!("/templates/{}", encode_label(&unknown_template)),
            body: None,
            resource_type: "Template",
            identifier: unknown_template,
        },
        MissingCase {
            case: "GetTemplate, a template of another partition".to_string(),
            method: "GET",
            target: format!("/templates/{}", encode_label(&other_partition_template)),
            body: None,
            resource_type: "Template",
            identifier: other_partition_template,
        },
        MissingCase {
            case: "GetDirectoryRegistration, a registration never created".to_string(),
            method: "GET",
            target: format!(
                "/directoryRegistrations/{}",
                encode_label(&unknown_registration)
            ),
            body: None,
            resource_type: "DirectoryRegistration",
            identifier: unknown_registration,
        },
        MissingCase {
            case: "CreateConnector, a directory carrying no registration".to_string(),
            method: "POST",
            target: "/connectors".to_string(),
            body: Some(create_connector_body(&unregistered_directory)),
            resource_type: "DirectoryRegistration",
            identifier: unregistered_directory,
        },
    ]
}

/// A connector and a directory registration, so each refusal below is a store that
/// holds resources answering about the one it does not hold. The registration comes
/// first: a connector's `DirectoryId` is resolved against it.
async fn populate(router: &MultiServiceRouter) {
    let (status, code, parsed) = call(
        router,
        "POST",
        "/directoryRegistrations",
        Some(&json!({ "DirectoryId": "d-0123456789" })),
    )
    .await;
    assert_eq!(
        status, 202,
        "CreateDirectoryRegistration must answer the modelled 202, \
         got {status} / {code:?} / {parsed}"
    );
    let (status, code, parsed) = call(
        router,
        "POST",
        "/connectors",
        Some(&json!({
            "DirectoryId": "d-0123456789",
            "CertificateAuthorityArn": format!(
                "arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate-authority/\
                 11111111-2222-3333-4444-555555555555"
            ),
            "VpcInformation": { "SecurityGroupIds": ["sg-0123456789abcdef0"] },
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "CreateConnector must answer the modelled 202, got {status} / {code:?} / {parsed}"
    );
}

#[tokio::test]
async fn a_resource_this_service_does_not_hold_is_refused_in_one_wording() {
    let router = router();
    populate(&router).await;

    for row in missing_cases() {
        let (status, code, parsed) =
            call(&router, row.method, &row.target, row.body.as_ref()).await;
        assert_eq!(
            status, 404,
            "case {:?}: a resource that does not exist is a 404, got {status} / {code:?} / {parsed}",
            row.case
        );
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {:?}: expected ResourceNotFoundException, got {code:?} / {parsed}",
            row.case
        );
        let expected = format!("{} was not found: {}.", row.resource_type, row.identifier);
        assert_eq!(
            parsed["Message"].as_str(),
            Some(expected.as_str()),
            "case {:?}: expected message {expected:?}, got {parsed}",
            row.case
        );
        assert_eq!(
            parsed["ResourceId"].as_str(),
            Some(row.identifier.as_str()),
            "case {:?}: the refusal must name the identifier as supplied, got {parsed}",
            row.case
        );
        assert_eq!(
            parsed["ResourceType"].as_str(),
            Some(row.resource_type),
            "case {:?}: the refusal must name the resource type, got {parsed}",
            row.case
        );
    }
}

/// The wording is the refusal of an ABSENT resource only: a resource this store holds
/// is read back through the same operations, so the sentence above is never a 404 for
/// something that exists.
#[tokio::test]
async fn a_resource_this_service_holds_is_read_back_instead_of_refused() {
    let router = router();
    populate(&router).await;

    let (status, _, page) = call(&router, "GET", "/connectors", None).await;
    assert_eq!(status, 200, "ListConnectors must answer 200, got {page}");
    let arn = page["Connectors"][0]["Arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(!arn.is_empty(), "the created connector must be listed, got {page}");

    let (status, code, parsed) =
        call(&router, "GET", &format!("/connectors/{}", encode_label(&arn)), None).await;
    assert_eq!(
        status, 200,
        "case {arn}: a connector this store holds is read back, got {status} / {code:?} / {parsed}"
    );
    assert_eq!(
        parsed["Connector"]["Arn"].as_str(),
        Some(arn.as_str()),
        "case {arn}: GetConnector must report the ARN it was addressed by, got {parsed}"
    );
}
