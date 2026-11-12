// SPDX-License-Identifier: CC0-1.0

//! The SSM documents Systems Manager PUBLISHES, end to end through `DescribeDocument`.
//!
//! The rule under test: a document Systems Manager publishes exists in every account and
//! Region without anybody creating it, so a read resolves it by name and reports it as
//! owned by Amazon and `Active`; the not-found error is for a name no document — created
//! or published — is held under.
//!
//! Every case goes through the front door a client uses — the shared harness in
//! `all/parameter_store.rs` ([`router`], [`call`], [`call_in_region`]) dispatches `POST /`
//! with `X-Amz-Target: AmazonSSM.<Operation>` through the multi-service router. Nothing
//! reaches into the state struct.
//!
//! The suites are tables. The rows of the first span the whole published catalogue —
//! document type, schema version, parameter count, and whether the body's step
//! preconditions restrict it to a platform — rather than the one runbook a recorded unit
//! happened to resolve; the later suites span Region, name spelling, ARN form and version
//! selector.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::parameter_store::{call, call_in_region, error_code, router, ACCOUNT, REGION};

/// The catalogue the service serves (`crate::state::PUBLISHED_DOCUMENTS`), so the table
/// below can be checked to cover every entry of it rather than a few.
const CATALOGUE: &[service_com_amazonaws_ssm_amazonssm::state::PublishedDocument] =
    service_com_amazonaws_ssm_amazonssm::state::PUBLISHED_DOCUMENTS;

/// The `PlatformType` values the model's enum declares, in the model's order
/// (`smithy_query(query="shape", shape="PlatformType")`), which is what a document whose
/// body restricts it to no platform is compatible with.
const PLATFORM_TYPES: &[&str] = &["Windows", "Linux", "MacOS"];

/// The `Owner` a published document reports: `Owner` is "the Amazon Web Services user
/// that created the document" (`DocumentDescription$Owner`), and these were created by
/// Amazon — the owner `ListDocuments`' `Owner` filter calls `Amazon`.
const AMAZON: &str = "Amazon";

/// One published-document row: the name a client names, and what the service must report
/// for it.
struct Published<'a> {
    case: &'a str,
    name: &'a str,
    document_type: &'a str,
    schema_version: &'a str,
    /// The parameters the document declares, in the order its body declares them.
    parameters: &'a [&'a str],
    /// The platforms the body's step preconditions restrict it to.
    platforms: &'a [&'a str],
}

/// The published catalogue, as a client sees it. The `AWS-*` Automation runbooks are the
/// ones an AWS Config remediation configuration names as its target; the `AWS-*` command
/// documents are the ones `SendCommand` names.
fn catalogue_rows() -> Vec<Published<'static>> {
    vec![
        Published {
            case: "the Automation runbook that configures bucket versioning",
            name: "AWS-ConfigureS3BucketVersioning",
            document_type: "Automation",
            schema_version: "0.3",
            parameters: &["AutomationAssumeRole", "BucketName", "VersioningState"],
            platforms: PLATFORM_TYPES,
        },
        Published {
            case: "an Automation runbook with six parameters",
            name: "AWS-ConfigureS3BucketPublicAccessBlock",
            document_type: "Automation",
            schema_version: "0.3",
            parameters: &[
                "AutomationAssumeRole",
                "BucketName",
                "BlockPublicAcls",
                "IgnorePublicAcls",
                "BlockPublicPolicy",
                "RestrictPublicBuckets",
            ],
            platforms: PLATFORM_TYPES,
        },
        Published {
            case: "an Automation runbook with two parameters",
            name: "AWS-DisableS3BucketPublicReadWrite",
            document_type: "Automation",
            schema_version: "0.3",
            parameters: &["AutomationAssumeRole", "S3BucketName"],
            platforms: PLATFORM_TYPES,
        },
        Published {
            case: "the Automation runbook that turns on default encryption",
            name: "AWS-EnableS3BucketEncryption",
            document_type: "Automation",
            schema_version: "0.3",
            parameters: &["AutomationAssumeRole", "BucketName", "SSEAlgorithm"],
            platforms: PLATFORM_TYPES,
        },
        Published {
            case: "the Automation runbook that publishes a notification",
            name: "AWS-PublishSNSNotification",
            document_type: "Automation",
            schema_version: "0.3",
            parameters: &["AutomationAssumeRole", "TopicArn", "Message"],
            platforms: PLATFORM_TYPES,
        },
        Published {
            case: "a command document restricted to two platforms",
            name: "AWS-RunShellScript",
            document_type: "Command",
            schema_version: "2.2",
            parameters: &["commands", "workingDirectory", "executionTimeout"],
            platforms: &["Linux", "MacOS"],
        },
        Published {
            case: "a command document restricted to one platform",
            name: "AWS-RunPowerShellScript",
            document_type: "Command",
            schema_version: "2.2",
            parameters: &["commands", "workingDirectory", "executionTimeout"],
            platforms: &["Windows"],
        },
        Published {
            case: "a command document restricted to no platform",
            name: "AWS-UpdateSSMAgent",
            document_type: "Command",
            schema_version: "2.2",
            parameters: &["version", "allowDowngrade"],
            platforms: PLATFORM_TYPES,
        },
    ]
}

/// `DescribeDocument`, returning `(status, body)`.
async fn describe(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "DescribeDocument", body).await
}

/// `DescribeDocument`'s `Document`, asserting the read succeeded.
async fn describe_ok(router: &MultiServiceRouter, case: &str, name: &str) -> Value {
    let (status, response) = describe(router, &json!({ "Name": name })).await;
    assert_eq!(
        status, 200,
        "case {case}: DescribeDocument {name} must succeed, got {status} {response}"
    );
    response["Document"].clone()
}

/// The `Name` of each reported `DocumentParameter`, in the order reported.
fn parameter_names(document: &Value) -> Vec<String> {
    document["Parameters"]
        .as_array()
        .map(|parameters| {
            parameters
                .iter()
                .map(|parameter| parameter["Name"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The hex Sha256 a document's `Hash` must be (`DocumentDescription$Hash`: "The Sha256 …
/// hash created by the system when the document was created"), computed here rather than
/// read from the implementation, so a wrong hash rule cannot pass.
fn content_hash(content: &str) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(content.as_bytes()))
}

/// A reported string list as owned strings.
fn string_list(document: &Value, member: &str) -> Vec<String> {
    document[member]
        .as_array()
        .map(|values| {
            values
                .iter()
                .map(|value| value.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Every document in the published catalogue resolves on a FRESH emulator, with no
/// create of any kind, and reports what its own body declares.
#[tokio::test]
async fn every_published_document_resolves_without_a_create() {
    let router = router();
    let rows = catalogue_rows();
    // The table is the whole catalogue, not a sample of it: a new entry has to be given
    // its row here, and a row may not name an entry the service does not serve.
    for entry in CATALOGUE {
        assert!(
            rows.iter().any(|row| row.name == entry.name),
            "the table must carry a row for published document {}",
            entry.name
        );
    }
    for row in &rows {
        assert!(
            CATALOGUE.iter().any(|entry| entry.name == row.name),
            "case {}: {} must be a document the service publishes",
            row.case,
            row.name
        );
    }

    for row in &rows {
        let document = describe_ok(&router, row.case, row.name).await;
        assert_eq!(
            document["Name"].as_str(),
            Some(row.name),
            "case {}: expected Name {}, got {document}",
            row.case,
            row.name
        );
        assert_eq!(
            document["Owner"].as_str(),
            Some(AMAZON),
            "case {}: a published document is owned by {AMAZON}, got {document}",
            row.case
        );
        assert_eq!(
            document["Status"].as_str(),
            Some("Active"),
            "case {}: a published document is already registered, got {document}",
            row.case
        );
        assert_eq!(
            document["DocumentType"].as_str(),
            Some(row.document_type),
            "case {}: expected DocumentType {}, got {document}",
            row.case,
            row.document_type
        );
        assert_eq!(
            document["SchemaVersion"].as_str(),
            Some(row.schema_version),
            "case {}: expected SchemaVersion {}, got {document}",
            row.case,
            row.schema_version
        );
        assert_eq!(
            parameter_names(&document),
            row.parameters,
            "case {}: expected the parameters its body declares, got {document}",
            row.case
        );
        assert_eq!(
            string_list(&document, "PlatformTypes"),
            row.platforms,
            "case {}: expected PlatformTypes {:?}, got {document}",
            row.case,
            row.platforms
        );
        assert!(
            document["Description"]
                .as_str()
                .is_some_and(|description| !description.is_empty()),
            "case {}: the description is derived from the body, got {document}",
            row.case
        );
        // One version, which both role selectors name.
        for member in ["DocumentVersion", "LatestVersion", "DefaultVersion"] {
            assert_eq!(
                document[member].as_str(),
                Some("1"),
                "case {}: expected {member} 1, got {document}",
                row.case
            );
        }
        assert_eq!(
            document["DocumentFormat"].as_str(),
            Some("JSON"),
            "case {}: expected the default document format, got {document}",
            row.case
        );
        assert_eq!(
            document["HashType"].as_str(),
            Some("Sha256"),
            "case {}: the system hash is Sha256, got {document}",
            row.case
        );
        let hash = document["Hash"].as_str().unwrap_or_default().to_string();
        assert!(
            hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "case {}: expected a hex Sha256 of the body, got {hash:?}",
            row.case
        );
        // And it is the hash OF THAT BODY, not merely a well-shaped digest.
        let body = CATALOGUE
            .iter()
            .find(|entry| entry.name == row.name)
            .map(|entry| entry.content)
            .unwrap_or_default();
        assert_eq!(
            hash,
            content_hash(body),
            "case {}: expected the hex Sha256 of the document's own body, got {hash}",
            row.case
        );
        assert!(
            document["CreatedDate"].as_f64().is_some_and(|date| date > 0.0),
            "case {}: a published document carries a creation date, got {document}",
            row.case
        );
    }
}

/// A published document is reachable through EVERY Region's endpoint, and by every name
/// spelling that resolves to it: the bare name, its full ARN, and the account-less ARN
/// form AWS publishes a public document under. An ARN naming another Region or another
/// account names nothing reachable here.
#[tokio::test]
async fn a_published_document_resolves_in_every_region_and_by_every_arn_spelling() {
    let router = router();
    let name = "AWS-ConfigureS3BucketVersioning";
    let mut hashes = Vec::new();
    for region in ["us-west-2", "eu-central-1", "ap-southeast-1"] {
        let (status, response) =
            call_in_region(&router, region, "DescribeDocument", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case region {region}: a published document exists in every Region, got {status} \
             {response}"
        );
        let document = response["Document"].clone();
        assert_eq!(
            document["Owner"].as_str(),
            Some(AMAZON),
            "case region {region}: expected Owner {AMAZON}, got {document}"
        );
        hashes.push(document["Hash"].as_str().unwrap_or_default().to_string());
    }
    assert!(
        hashes.windows(2).all(|pair| pair[0] == pair[1]),
        "one document means one body: expected the same Hash in every Region, got {hashes:?}"
    );

    // (case, Name, resolves)
    let spellings: &[(&str, String, bool)] = &[
        ("the bare name", name.to_string(), true),
        (
            "the full ARN of this account and Region",
            format!("arn:aws:ssm:{REGION}:{ACCOUNT}:document/{name}"),
            true,
        ),
        (
            "the account-less ARN form of a published document",
            format!("arn:aws:ssm:{REGION}::document/{name}"),
            true,
        ),
        (
            "an ARN naming another Region",
            format!("arn:aws:ssm:eu-west-1:{ACCOUNT}:document/{name}"),
            false,
        ),
        (
            "an ARN naming another account",
            format!("arn:aws:ssm:{REGION}:210987654321:document/{name}"),
            false,
        ),
        (
            "an ARN of another service",
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:document/{name}"),
            false,
        ),
    ];
    for (case, spelling, resolves) in spellings {
        let (status, response) = describe(&router, &json!({ "Name": spelling })).await;
        if *resolves {
            assert_eq!(
                status, 200,
                "case {case}: {spelling} must resolve the published document, got {status} \
                 {response}"
            );
            assert_eq!(
                response["Document"]["Name"].as_str(),
                Some(name),
                "case {case}: expected Name {name}, got {response}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case}: {spelling} names nothing reachable here, got {status} {response}"
            );
            assert_eq!(
                error_code(&response),
                "InvalidDocument",
                "case {case}: expected InvalidDocument, got {response}"
            );
        }
    }
}

/// The not-found error is for a name no document is held under — a catalogue name is
/// resolved as written, and nothing invents a document for an unpublished `AWS-*` name.
#[tokio::test]
async fn a_name_no_document_is_held_under_is_not_found() {
    let router = router();
    // (case, Name)
    let misses: &[(&str, &str)] = &[
        (
            "an AWS-prefixed name the catalogue does not hold",
            "AWS-ConfigureS3BucketVersioningThatIsNotPublished",
        ),
        (
            "a published name in another case: a document name is case-sensitive",
            "aws-configures3bucketversioning",
        ),
        (
            "a published name with a trailing dot",
            "AWS-ConfigureS3BucketVersioning.",
        ),
        (
            "another reserved prefix the catalogue holds nothing under",
            "AWSConfigRemediation-NothingHere",
        ),
        ("a name no caller created", "MyOwnDocument"),
    ];
    for (case, name) in misses {
        let (status, response) = describe(&router, &json!({ "Name": name })).await;
        assert_eq!(
            status, 400,
            "case {case}: {name} names no document, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "InvalidDocument",
            "case {case}: expected InvalidDocument, got {response}"
        );
    }
}

/// A published document holds one version, which both role selectors name; any other
/// version or version name names no version of it.
#[tokio::test]
async fn a_published_documents_versions_are_the_one_it_holds() {
    let router = router();
    let name = "AWS-RunShellScript";
    /// One version-selector row: what the request selects, and the error code it must be
    /// refused with, or `None` when it must resolve.
    struct Selector<'a> {
        case: &'a str,
        version: Option<&'a str>,
        version_name: Option<&'a str>,
        expected: Option<&'a str>,
    }
    let cases = &[
        Selector {
            case: "no selector",
            version: None,
            version_name: None,
            expected: None,
        },
        Selector {
            case: "the latest selector",
            version: Some("$LATEST"),
            version_name: None,
            expected: None,
        },
        Selector {
            case: "the default selector",
            version: Some("$DEFAULT"),
            version_name: None,
            expected: None,
        },
        Selector {
            case: "the one version it holds",
            version: Some("1"),
            version_name: None,
            expected: None,
        },
        Selector {
            case: "a version it does not hold",
            version: Some("2"),
            version_name: None,
            expected: Some("InvalidDocumentVersion"),
        },
        Selector {
            case: "a high version it does not hold",
            version: Some("97"),
            version_name: None,
            expected: Some("InvalidDocumentVersion"),
        },
        Selector {
            case: "a version name it does not carry",
            version: None,
            version_name: Some("1.0"),
            expected: Some("InvalidDocumentVersion"),
        },
    ];
    for Selector {
        case,
        version,
        version_name,
        expected,
    } in cases
    {
        let mut body = json!({ "Name": name });
        if let Some(version) = version {
            body["DocumentVersion"] = json!(version);
        }
        if let Some(version_name) = version_name {
            body["VersionName"] = json!(version_name);
        }
        let (status, response) = describe(&router, &body).await;
        match expected {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case}: must resolve, got {status} {response}"
                );
                assert_eq!(
                    response["Document"]["DocumentVersion"].as_str(),
                    Some("1"),
                    "case {case}: expected the one version it holds, got {response}"
                );
            }
            Some(code) => {
                assert_eq!(
                    status, 400,
                    "case {case}: must be refused, got {status} {response}"
                );
                assert_eq!(
                    &error_code(&response),
                    code,
                    "case {case}: expected {code}, got {response}"
                );
            }
        }
    }
}

/// The catalogue is READ-ONLY and reads of it are stable: two reads report the same
/// document, a `CreateDocument` cannot take a published name, and the document the
/// account creates reports the ACCOUNT as its owner — so `Owner` is a fact of each
/// document rather than one answer for all of them.
#[tokio::test]
async fn the_published_catalogue_is_read_only_and_stable() {
    let router = router();
    let name = "AWS-EnableS3BucketEncryption";
    let first = describe_ok(&router, "the first read", name).await;

    // A create may not take a published name.
    let (status, response) = call(
        &router,
        "CreateDocument",
        &json!({
            "Name": name,
            "Content": "{\"schemaVersion\":\"0.3\",\"description\":\"mine\",\"mainSteps\":[]}",
            "DocumentType": "Automation",
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a published name is reserved, so the create must be refused, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "ValidationException",
        "the reserved-prefix rule is what refuses it, got {response}"
    );

    let second = describe_ok(&router, "the read after the refused create", name).await;
    assert_eq!(
        second, first,
        "a published document is unchanged by a read and by a refused create: expected \
         {first}, got {second}"
    );

    // A document the account creates is owned by the account, not by Amazon.
    let mine = json!({
        "Name": "MyVersioningRunbook",
        "Content": "{\"schemaVersion\":\"0.3\",\"description\":\"mine\",\"mainSteps\":[]}",
        "DocumentType": "Automation",
    });
    let (status, response) = call(&router, "CreateDocument", &mine).await;
    assert_eq!(
        status, 200,
        "a name of its own must be creatable, got {status} {response}"
    );
    let created = describe_ok(&router, "a document the account created", "MyVersioningRunbook").await;
    assert_eq!(
        created["Owner"].as_str(),
        Some(ACCOUNT),
        "a document the account created is owned by the account, got {created}"
    );
    assert_eq!(
        first["Owner"].as_str(),
        Some(AMAZON),
        "and a published one is owned by {AMAZON}, got {first}"
    );
}
