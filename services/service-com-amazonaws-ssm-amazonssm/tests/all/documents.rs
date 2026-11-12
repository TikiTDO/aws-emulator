// SPDX-License-Identifier: CC0-1.0

//! SSM documents end to end: `CreateDocument` registers, `DescribeDocument` reports.
//!
//! Every case goes through the front door a client uses — the shared harness in
//! `all/parameter_store.rs` ([`router`], [`call`], [`call_in_region`]) dispatches
//! `POST /` with `X-Amz-Target: AmazonSSM.<Operation>` through the multi-service
//! router. Nothing reaches into the state struct, so a response that is not computed
//! from persisted state cannot pass: every read follows a write.
//!
//! The suites are tables. Each row is one input along the axis the rule under test
//! generalises over — document type, document format, schema version, content shape,
//! parameter count and default form, platform precondition, name prefix, content size,
//! version selector, Region — rather than the single document a recorded unit happened
//! to create.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::parameter_store::{call, call_in_region, error_code, router, ACCOUNT, REGION};

/// The transient window a created document spends in `Creating`
/// (`crate::state::DOCUMENT_TRANSITION_DELAY`), which the service stores as a due
/// instant rather than sleeping for.
const TRANSITION_DELAY: Duration =
    service_com_amazonaws_ssm_amazonssm::state::DOCUMENT_TRANSITION_DELAY;

/// The `DocumentFormat` values the model's enum declares
/// (`smithy_query(query="shape", shape="DocumentFormat")`).
const DOCUMENT_FORMATS: &[&str] = &["JSON", "TEXT", "YAML"];

/// The `PlatformType` values the model's enum declares, in the order it declares them
/// (`smithy_query(query="shape", shape="PlatformType")`).
const PLATFORM_TYPES: &[&str] = &["Windows", "Linux", "MacOS"];

/// The document-name prefixes `CreateDocument`'s `Name` doc reserves: "These are
/// reserved by Amazon Web Services for use as document name prefixes: aws amazon amzn
/// AWSEC2 AWSConfigRemediation AWSSupport".
const RESERVED_PREFIXES: &[&str] = &[
    "aws",
    "amazon",
    "amzn",
    "AWSEC2",
    "AWSConfigRemediation",
    "AWSSupport",
];

/// "The content of the document must not exceed 64KB" (`CreateDocument`'s `Content`
/// doc), reported by `MaxDocumentSizeExceeded` ("The size limit of a document is 64
/// KB").
const MAX_CONTENT: usize = 64 * 1024;

/// A `Command` document: schema version 2.2, two parameters (a scalar default and a
/// list default), and one step whose precondition restricts it to Linux.
const COMMAND_JSON: &str = r#"{
  "schemaVersion": "2.2",
  "description": "Echo a message on a managed node",
  "parameters": {
    "Message": { "type": "String", "description": "what to echo", "default": "hello" },
    "Commands": { "type": "StringList", "default": ["echo one", "echo two"] }
  },
  "mainSteps": [
    {
      "action": "aws:runShellScript",
      "name": "runShell",
      "precondition": { "StringEquals": ["platformType", "Linux"] },
      "inputs": { "runCommand": ["echo hi"] }
    }
  ]
}"#;

/// The same kind of document written in YAML, with an `Automation` schema version and
/// no platform precondition at all.
const AUTOMATION_YAML: &str = r#"schemaVersion: '0.3'
description: Remediate a noncompliant resource
assumeRole: '{{ AutomationAssumeRole }}'
parameters:
  AutomationAssumeRole:
    type: String
    description: the role Automation assumes
  BucketName:
    type: String
mainSteps:
  - name: putBucketLogging
    action: 'aws:executeAwsApi'
    inputs:
      Service: s3
      Api: PutBucketLogging
"#;

/// A `Session` document: schema version 1.0, no parameters, no steps.
const SESSION_JSON: &str = r#"{
  "schemaVersion": "1.0",
  "description": "Start an interactive shell",
  "sessionType": "Standard_Stream",
  "inputs": { "idleSessionTimeout": "20" }
}"#;

/// A `Command` document restricted to two platforms by its step preconditions, in the
/// order the model does NOT list them, so the reported order is the model's.
const TWO_PLATFORM_YAML: &str = r#"schemaVersion: '2.2'
description: Run on Windows and MacOS only
mainSteps:
  - name: runOnWindows
    action: 'aws:runPowerShellScript'
    precondition:
      StringEquals: [platformType, Windows]
    inputs:
      runCommand: ['Write-Host hi']
  - name: runOnMac
    action: 'aws:runShellScript'
    precondition:
      StringEquals: [platformType, MacOS]
    inputs:
      runCommand: ['echo hi']
"#;

/// A Change Calendar entry: `TEXT` content, which carries no document tree at all.
const CHANGE_CALENDAR_TEXT: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//emulator//test//EN\r\nEND:VCALENDAR";

/// An AppConfig schema document: a document type whose content carries no Systems
/// Manager schema version, so the content is persisted as the caller wrote it.
const APPCONFIG_SCHEMA_JSON: &str = r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "description": "An AppConfig configuration schema",
  "type": "object"
}"#;

/// `CreateDocument` with `body`, returning `(status, body)`.
async fn create(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "CreateDocument", body).await
}

/// `CreateDocument`, asserting it succeeded, and returning its `DocumentDescription`.
async fn create_ok(router: &MultiServiceRouter, case: &str, body: &Value) -> Value {
    let (status, response) = create(router, body).await;
    assert_eq!(
        status, 200,
        "case {case}: CreateDocument must succeed, got {status} {response}"
    );
    response["DocumentDescription"].clone()
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

/// The hex Sha256 the service must report as a document's `Hash`
/// (`DocumentDescription$Hash`: "The Sha256 … hash created by the system when the
/// document was created").
fn content_hash(content: &str) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(content.as_bytes()))
}

/// A description with its `Status` and its classification members dropped, so a create's
/// report and a later read's can be compared: the clock-owed status move would make the
/// comparison a race, and a DESCRIBE carries `Category` and `CategoryEnum` where the
/// create's own response carries neither (asserted on their own below).
fn comparable(description: &Value) -> Value {
    let mut copy = description.clone();
    if let Some(map) = copy.as_object_mut() {
        map.remove("Status");
        map.remove("Category");
        map.remove("CategoryEnum");
    }
    copy
}

/// The `Name` member of each reported `DocumentParameter`, in the order reported.
fn parameter_names(description: &Value) -> Vec<String> {
    description["Parameters"]
        .as_array()
        .map(|parameters| {
            parameters
                .iter()
                .map(|parameter| parameter["Name"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// One round-trip row: a document form, and what the service must derive from it.
struct RoundTrip<'a> {
    case: &'a str,
    name: &'a str,
    /// `None` leaves `DocumentType` to the service's default.
    document_type: Option<&'a str>,
    /// `None` leaves `DocumentFormat` to the documented `JSON` default.
    format: Option<&'a str>,
    content: &'a str,
    expected_type: &'a str,
    expected_format: &'a str,
    schema_version: Option<&'a str>,
    description: Option<&'a str>,
    parameters: &'a [&'a str],
    platforms: &'a [&'a str],
}

/// Every document form round-trips: what a create persisted is what a later describe
/// reports, and everything beyond the request's own members is DERIVED from the content
/// — the schema version, the description, the parameters, the platform types, the hash.
#[tokio::test]
async fn create_then_describe_reports_what_was_persisted() {
    let router = router();
    let cases = vec![
        RoundTrip {
            case: "a JSON Command document with a Linux precondition",
            name: "EchoMessage",
            document_type: None,
            format: None,
            content: COMMAND_JSON,
            expected_type: "Command",
            expected_format: "JSON",
            schema_version: Some("2.2"),
            description: Some("Echo a message on a managed node"),
            parameters: &["Message", "Commands"],
            platforms: &["Linux"],
        },
        RoundTrip {
            case: "a YAML Automation document with no precondition",
            name: "RemediateBucket",
            document_type: Some("Automation"),
            format: Some("YAML"),
            content: AUTOMATION_YAML,
            expected_type: "Automation",
            expected_format: "YAML",
            schema_version: Some("0.3"),
            description: Some("Remediate a noncompliant resource"),
            parameters: &["AutomationAssumeRole", "BucketName"],
            platforms: PLATFORM_TYPES,
        },
        RoundTrip {
            case: "a JSON Session document with no parameters",
            name: "StartShell",
            document_type: Some("Session"),
            format: Some("JSON"),
            content: SESSION_JSON,
            expected_type: "Session",
            expected_format: "JSON",
            schema_version: Some("1.0"),
            description: Some("Start an interactive shell"),
            parameters: &[],
            platforms: PLATFORM_TYPES,
        },
        RoundTrip {
            case: "two platform preconditions",
            name: "RunOnTwoPlatforms",
            document_type: Some("Command"),
            format: Some("YAML"),
            content: TWO_PLATFORM_YAML,
            expected_type: "Command",
            expected_format: "YAML",
            schema_version: Some("2.2"),
            description: Some("Run on Windows and MacOS only"),
            parameters: &[],
            platforms: &["Windows", "MacOS"],
        },
        RoundTrip {
            case: "TEXT content, which carries no document tree",
            name: "MyChangeCalendar",
            document_type: Some("ChangeCalendar"),
            format: Some("TEXT"),
            content: CHANGE_CALENDAR_TEXT,
            expected_type: "ChangeCalendar",
            expected_format: "TEXT",
            schema_version: None,
            description: None,
            parameters: &[],
            platforms: &[],
        },
        RoundTrip {
            case: "a document type that declares no Systems Manager schema version",
            name: "MyAppConfigSchema",
            document_type: Some("ApplicationConfigurationSchema"),
            format: Some("JSON"),
            content: APPCONFIG_SCHEMA_JSON,
            expected_type: "ApplicationConfigurationSchema",
            expected_format: "JSON",
            schema_version: None,
            description: Some("An AppConfig configuration schema"),
            parameters: &[],
            platforms: PLATFORM_TYPES,
        },
    ];
    for format in DOCUMENT_FORMATS {
        assert!(
            cases.iter().any(|row| row.expected_format == *format),
            "the round-trip table must cover DocumentFormat {format}"
        );
    }

    for row in cases {
        let case = row.case;
        let mut body = json!({ "Name": row.name, "Content": row.content });
        if let Some(document_type) = row.document_type {
            body["DocumentType"] = json!(document_type);
        }
        if let Some(format) = row.format {
            body["DocumentFormat"] = json!(format);
        }
        let created = create_ok(&router, case, &body).await;

        assert_eq!(
            created["Name"],
            json!(row.name),
            "case {case}: expected Name {}, got {created}",
            row.name
        );
        assert_eq!(
            created["DocumentType"],
            json!(row.expected_type),
            "case {case}: expected DocumentType {}, got {created}",
            row.expected_type
        );
        assert_eq!(
            created["DocumentFormat"],
            json!(row.expected_format),
            "case {case}: expected DocumentFormat {}, got {created}",
            row.expected_format
        );
        assert_eq!(
            created["SchemaVersion"],
            row.schema_version.map_or(Value::Null, |version| json!(version)),
            "case {case}: expected SchemaVersion {:?}, got {created}",
            row.schema_version
        );
        assert_eq!(
            created["Description"],
            row.description.map_or(Value::Null, |text| json!(text)),
            "case {case}: expected Description {:?}, got {created}",
            row.description
        );
        let expected_parameters: Vec<String> =
            row.parameters.iter().map(|name| (*name).to_string()).collect();
        assert_eq!(
            parameter_names(&created),
            expected_parameters,
            "case {case}: expected the content's parameters {:?}, got {created}",
            row.parameters
        );
        assert_eq!(
            created["PlatformTypes"],
            if row.platforms.is_empty() {
                Value::Null
            } else {
                json!(row.platforms)
            },
            "case {case}: expected PlatformTypes {:?}, got {created}",
            row.platforms
        );
        assert_eq!(
            created["Hash"],
            json!(content_hash(row.content)),
            "case {case}: Hash must be the Sha256 of the content, got {created}"
        );
        assert_eq!(
            created["HashType"],
            json!("Sha256"),
            "case {case}: expected HashType Sha256, got {created}"
        );
        assert_eq!(
            created["Owner"],
            json!(ACCOUNT),
            "case {case}: expected Owner {ACCOUNT}, got {created}"
        );
        assert_eq!(
            created["Status"],
            json!("Creating"),
            "case {case}: a created document starts Creating, got {created}"
        );
        for member in ["DocumentVersion", "LatestVersion", "DefaultVersion"] {
            assert_eq!(
                created[member],
                json!("1"),
                "case {case}: expected {member} 1 for a document's first version, got {created}"
            );
        }
        assert!(
            created["CreatedDate"].is_number(),
            "case {case}: expected a CreatedDate, got {created}"
        );
        // None of these rows carries a tag, and an untagged document reports its tag set
        // as the EMPTY list the modelled `TagList` admits rather than as no member.
        assert_eq!(
            created["Tags"],
            json!([]),
            "case {case}: an untagged document reports an empty Tags list, got {created}"
        );
        // The create's own response carries no classification members at all.
        for member in ["Category", "CategoryEnum"] {
            assert_eq!(
                created[member],
                Value::Null,
                "case {case}: a create reports no {member}, got {created}"
            );
        }
        // A review is the change-template approval workflow, so no other document type
        // reports one.
        assert_eq!(
            created["ReviewStatus"],
            Value::Null,
            "case {case}: only a change template carries a ReviewStatus, got {created}"
        );

        let read = describe_ok(&router, case, row.name).await;
        assert_eq!(
            comparable(&read),
            comparable(&created),
            "case {case}: DescribeDocument must report what CreateDocument persisted, got {read}"
        );
        // A document nothing classified carries both classification members as empty
        // lists when it is DESCRIBED (`CategoryList` / `CategoryEnumList`, length 0..3).
        for member in ["Category", "CategoryEnum"] {
            assert_eq!(
                read[member],
                json!([]),
                "case {case}: a describe reports an empty {member} list, got {read}"
            );
        }
    }

    // A change template is the one type whose versions are reviewed.
    let template = create_ok(
        &router,
        "a change template",
        &json!({
            "Name": "MyChangeTemplate",
            "DocumentType": "Automation.ChangeTemplate",
            "Content": AUTOMATION_YAML,
            "DocumentFormat": "YAML",
        }),
    )
    .await;
    assert_eq!(
        template["ReviewStatus"],
        json!("NOT_REVIEWED"),
        "a change template nobody reviewed is NOT_REVIEWED, got {template}"
    );
}

/// The optional members a create carries are persisted and reported back, and the
/// attachment names a read reports are the ones the create named.
#[tokio::test]
async fn the_optional_members_of_a_create_round_trip() {
    let router = router();
    let body = json!({
        "Name": "FullyDescribedDocument",
        "Content": COMMAND_JSON,
        "DisplayName": "Fully described document",
        "VersionName": "Release12.1",
        "TargetType": "/AWS::EC2::Instance",
        "Tags": [
            { "Key": "Environment", "Value": "Production" },
            { "Key": "OS", "Value": "Windows" },
        ],
        "Requires": [{ "Name": "MyAppConfigSchema", "Version": "1" }],
        "Attachments": [
            { "Key": "S3FileUrl", "Values": ["s3://amzn-s3-demo-bucket/my-prefix/my-file.py"] },
            { "Key": "SourceUrl", "Values": ["s3://amzn-s3-demo-bucket/my-prefix"], "Name": "named-file.py" },
        ],
    });
    let created = create_ok(&router, "every optional member", &body).await;
    let read = describe_ok(&router, "every optional member", "FullyDescribedDocument").await;

    for (member, expected) in [
        ("DisplayName", json!("Fully described document")),
        ("VersionName", json!("Release12.1")),
        ("TargetType", json!("/AWS::EC2::Instance")),
        (
            "Tags",
            json!([
                { "Key": "Environment", "Value": "Production" },
                { "Key": "OS", "Value": "Windows" },
            ]),
        ),
        ("Requires", json!([{ "Name": "MyAppConfigSchema", "Version": "1" }])),
        (
            "AttachmentsInformation",
            json!([{ "Name": "my-file.py" }, { "Name": "named-file.py" }]),
        ),
    ] {
        assert_eq!(
            created[member], expected,
            "case create {member}: expected {expected}, got {created}"
        );
        assert_eq!(
            read[member], expected,
            "case describe {member}: expected {expected}, got {read}"
        );
    }

    // "This value is unique across all versions of a document": the version name the
    // create supplied selects the version, and any other names none.
    let (status, response) = describe(
        &router,
        &json!({ "Name": "FullyDescribedDocument", "VersionName": "Release12.1" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the stored VersionName must select the version, got {status} {response}"
    );
    let (status, response) = describe(
        &router,
        &json!({ "Name": "FullyDescribedDocument", "VersionName": "Release9.9" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "InvalidDocumentVersion"),
        "a VersionName the document doesn't carry names no version, got {status} {response}"
    );
}

/// A created document is `Creating` until the clock makes its move to `Active` due, and
/// `Active` after — materialised from the stored due instant, with nothing sleeping.
#[tokio::test]
async fn a_created_document_becomes_active_once_the_transition_is_due() {
    let router = router();
    let started = std::time::Instant::now();
    let created = create_ok(
        &router,
        "the transient window",
        &json!({ "Name": "TransitionDocument", "Content": COMMAND_JSON }),
    )
    .await;
    assert_eq!(
        created["Status"],
        json!("Creating"),
        "a create reports the transient status, got {created}"
    );
    let read = describe_ok(&router, "before the transition is due", "TransitionDocument").await;
    // Only assert the pre-due status while the window is demonstrably still open: on a
    // loaded machine the round trip above can itself outlast it, and the point of the
    // case is the move, not how fast this process runs.
    if started.elapsed() < TRANSITION_DELAY {
        assert_eq!(
            read["Status"],
            json!("Creating"),
            "a read before the transition is due still reports Creating, got {read}"
        );
    }

    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(250)).await;

    let read = describe_ok(&router, "after the transition is due", "TransitionDocument").await;
    assert_eq!(
        read["Status"],
        json!("Active"),
        "a read past the due instant reports Active, got {read}"
    );
}

/// A document name is unique within a Region: a second create of one name is refused
/// and changes nothing, while the same name in another Region is a different document.
#[tokio::test]
async fn a_name_a_region_already_holds_is_refused() {
    let router = router();
    for name in ["FirstDocument", "SecondDocument"] {
        create_ok(&router, name, &json!({ "Name": name, "Content": COMMAND_JSON })).await;
    }
    for name in ["FirstDocument", "SecondDocument"] {
        // Different content from the first create, so the refusal cannot be mistaken
        // for content validation and the check below shows nothing was replaced.
        let (status, response) = create(
            &router,
            &json!({ "Name": name, "Content": "{\"schemaVersion\": \"2.2\"}" }),
        )
        .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "DocumentAlreadyExists"),
            "case {name}: a second create of one name is refused, got {status} {response}"
        );
        // The refused create must not have replaced what the first one persisted.
        let read = describe_ok(&router, name, name).await;
        assert_eq!(
            read["Hash"],
            json!(content_hash(COMMAND_JSON)),
            "case {name}: a refused create must leave the stored content alone, got {read}"
        );
    }

    // Another Region's documents are other documents: the same name is free there and
    // this Region's document is not visible from it.
    let (status, response) = call_in_region(
        &router,
        "eu-west-1",
        "CreateDocument",
        &json!({ "Name": "FirstDocument", "Content": SESSION_JSON, "DocumentType": "Session" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the same name in another Region is another document, got {status} {response}"
    );
    let (status, response) = call_in_region(
        &router,
        "eu-central-1",
        "DescribeDocument",
        &json!({ "Name": "FirstDocument" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "InvalidDocument"),
        "a Region that holds no such document reports none, got {status} {response}"
    );
}

/// The reserved prefixes are refused whatever their case, and a name that merely
/// CONTAINS one is accepted: the rule is about the prefix, not the substring.
#[tokio::test]
async fn a_reserved_name_prefix_is_refused() {
    let router = router();
    for prefix in RESERVED_PREFIXES {
        for name in [
            format!("{prefix}-MyDocument"),
            format!("{}-MyDocument", prefix.to_uppercase()),
            format!("{}-MyDocument", prefix.to_lowercase()),
        ] {
            let (status, response) =
                create(&router, &json!({ "Name": name, "Content": COMMAND_JSON })).await;
            assert_eq!(
                (status, error_code(&response).as_str()),
                (400, "ValidationException"),
                "case {name}: a reserved document name prefix is refused, got {status} {response}"
            );
        }
    }
    // Not a reserved prefix: the reserved string appears later in the name, or the head
    // of the name only resembles one.
    for name in ["MyDocument-aws", "Awesome.Document", "my-amzn-document"] {
        let (status, response) =
            create(&router, &json!({ "Name": name, "Content": COMMAND_JSON })).await;
        assert_eq!(
            status, 200,
            "case {name}: only a reserved PREFIX is refused, got {status} {response}"
        );
    }
}

/// Content that is not a document in the format the request declared is refused with
/// `InvalidDocumentContent`, and persists nothing.
#[tokio::test]
async fn content_that_isnt_a_document_is_refused() {
    let router = router();
    let aliased = "schemaVersion: '2.2'\nanchor: &a [a, a, a]\nalias: *a\n";
    // A parameter default nested past the depth this service reads (64 levels).
    let over_nested = format!(
        "{{\"schemaVersion\": \"2.2\", \"parameters\": {{\"Deep\": {{\"type\": \"String\", \
         \"default\": {}{}}}}}}}",
        "[".repeat(70),
        "]".repeat(70)
    );
    let cases: Vec<(&str, &str, &str)> = vec![
        ("JSON that doesn't parse", "JSON", "{\"schemaVersion\": "),
        ("YAML that doesn't parse", "YAML", "schemaVersion: '2.2'\nparameters: [oops\n"),
        ("a JSON array rather than a document", "JSON", "[]"),
        ("a bare scalar", "YAML", "just-a-string"),
        (
            "parameters that are not a mapping",
            "JSON",
            "{\"schemaVersion\": \"2.2\", \"parameters\": []}",
        ),
        (
            "a parameter definition that is not a mapping",
            "JSON",
            "{\"schemaVersion\": \"2.2\", \"parameters\": {\"Message\": \"String\"}}",
        ),
        ("no schemaVersion at all", "JSON", "{\"description\": \"nothing else\"}"),
        ("a YAML alias", "YAML", aliased),
        ("a parameter default nested too deep", "JSON", over_nested.as_str()),
    ];
    for (case, format, content) in cases {
        let name = "RefusedContentDocument";
        let (status, response) = create(
            &router,
            &json!({ "Name": name, "Content": content, "DocumentFormat": format }),
        )
        .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "InvalidDocumentContent"),
            "case {case:?}: content that isn't a document is refused, got {status} {response}"
        );
        let (status, response) = describe(&router, &json!({ "Name": name })).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "InvalidDocument"),
            "case {case:?}: a refused create persists nothing, got {status} {response}"
        );
    }
}

/// A schema version the document type does not support is refused with
/// `InvalidDocumentSchemaVersion`; every supported one is accepted and reported back.
#[tokio::test]
async fn an_unsupported_schema_version_is_refused() {
    let router = router();
    // The schema versions Systems Manager publishes per document type (the
    // "schemaVersion" rules in Systems Manager Documents; `supported_schema_versions`
    // in `src/state.rs` carries the same table).
    let cases: &[(&str, &[&str], &[&str])] = &[
        ("Command", &["1.2", "2.0", "2.2"], &["0.3", "1.0", "3.0"]),
        ("Policy", &["1.0", "2.0"], &["2.2", "0.3"]),
        ("Session", &["1.0"], &["2.2"]),
        ("Package", &["2.0"], &["1.0"]),
        ("Automation", &["0.3"], &["2.2", "0.4"]),
        ("Automation.ChangeTemplate", &["0.3"], &["2.2"]),
    ];
    let mut minted = 0;
    for (document_type, supported, unsupported) in cases {
        for version in *supported {
            minted += 1;
            let name = format!("SchemaDocument{minted}");
            let content = format!("{{\"schemaVersion\": \"{version}\"}}");
            let case = format!("{document_type} {version}");
            let created = create_ok(
                &router,
                &case,
                &json!({ "Name": name, "Content": content, "DocumentType": document_type }),
            )
            .await;
            assert_eq!(
                created["SchemaVersion"],
                json!(version),
                "case {case}: a supported schema version is reported back, got {created}"
            );
        }
        for version in *unsupported {
            minted += 1;
            let name = format!("SchemaDocument{minted}");
            let content = format!("{{\"schemaVersion\": \"{version}\"}}");
            let case = format!("{document_type} {version}");
            let (status, response) = create(
                &router,
                &json!({ "Name": name, "Content": content, "DocumentType": document_type }),
            )
            .await;
            assert_eq!(
                (status, error_code(&response).as_str()),
                (400, "InvalidDocumentSchemaVersion"),
                "case {case}: an unsupported schema version is refused, got {status} {response}"
            );
        }
    }
    // A document type whose content carries no Systems Manager schema version is not
    // measured against that table at all.
    let created = create_ok(
        &router,
        "CloudFormation 9.9",
        &json!({
            "Name": "CloudFormationDocument",
            "Content": "{\"schemaVersion\": \"9.9\"}",
            "DocumentType": "CloudFormation",
        }),
    )
    .await;
    assert_eq!(
        created["SchemaVersion"],
        json!("9.9"),
        "a type with no schema table reports what the content declares, got {created}"
    );
}

/// The published content-size limit is a bound: content AT the limit is accepted and
/// content one byte past it is refused with `MaxDocumentSizeExceeded`.
#[tokio::test]
async fn content_past_the_published_size_limit_is_refused() {
    let router = router();
    let envelope = "{\"schemaVersion\": \"2.2\", \"description\": \"\"}";
    let padding = |total: usize| {
        let mut content = String::with_capacity(total);
        content.push_str("{\"schemaVersion\": \"2.2\", \"description\": \"");
        content.push_str(&"p".repeat(total - envelope.len()));
        content.push_str("\"}");
        content
    };
    for (case, size, expected) in [
        ("one byte under the limit", MAX_CONTENT - 1, None),
        ("exactly the limit", MAX_CONTENT, None),
        (
            "one byte past the limit",
            MAX_CONTENT + 1,
            Some("MaxDocumentSizeExceeded"),
        ),
    ] {
        let content = padding(size);
        assert_eq!(
            content.len(),
            size,
            "case {case}: the test must submit exactly {size} bytes"
        );
        let name = format!("SizedDocument{size}");
        let (status, response) =
            create(&router, &json!({ "Name": name, "Content": content })).await;
        match expected {
            None => assert_eq!(
                status, 200,
                "case {case}: {size} bytes of content is within the limit, got {status} {response}"
            ),
            Some(code) => assert_eq!(
                (status, error_code(&response).as_str()),
                (400, code),
                "case {case}: {size} bytes of content is past the limit, got {status} {response}"
            ),
        }
    }
}

/// A read resolves the document by bare name or by ARN, through the version selectors
/// the model declares, and reports `InvalidDocument` / `InvalidDocumentVersion` for
/// what names nothing.
#[tokio::test]
async fn a_read_resolves_a_document_by_name_or_arn_and_version() {
    let router = router();
    create_ok(
        &router,
        "the read target",
        &json!({ "Name": "ReadMeDocument", "Content": COMMAND_JSON }),
    )
    .await;
    let arn = format!("arn:aws:ssm:{REGION}:{ACCOUNT}:document/ReadMeDocument");

    for name in ["ReadMeDocument", arn.as_str()] {
        for version in [None, Some("$LATEST"), Some("$DEFAULT"), Some("1")] {
            let case = format!("{name} {version:?}");
            let mut body = json!({ "Name": name });
            if let Some(version) = version {
                body["DocumentVersion"] = json!(version);
            }
            let (status, response) = describe(&router, &body).await;
            assert_eq!(
                status, 200,
                "case {case}: the document resolves, got {status} {response}"
            );
            assert_eq!(
                response["Document"]["Name"],
                json!("ReadMeDocument"),
                "case {case}: the stored name is reported, got {response}"
            );
        }
        // A version the document does not carry.
        for version in ["2", "17"] {
            let case = format!("{name} version {version}");
            let (status, response) = describe(
                &router,
                &json!({ "Name": name, "DocumentVersion": version }),
            )
            .await;
            assert_eq!(
                (status, error_code(&response).as_str()),
                (400, "InvalidDocumentVersion"),
                "case {case}: a version the document has not got is refused, got {status} {response}"
            );
        }
    }

    // Names that address no document in this Region and account.
    let other_region = format!("arn:aws:ssm:eu-west-1:{ACCOUNT}:document/ReadMeDocument");
    let other_account = format!("arn:aws:ssm:{REGION}:111122223333:document/ReadMeDocument");
    for (case, name) in [
        ("a document nothing created", "NoSuchDocument"),
        ("an ARN of another Region", other_region.as_str()),
        ("an ARN of another account", other_account.as_str()),
        ("an ARN of another service", "arn:aws:s3:::amzn-s3-demo-bucket"),
        ("an ssm ARN of another resource type", "arn:aws:ssm:us-west-2:000000000000:parameter/p"),
    ] {
        let (status, response) = describe(&router, &json!({ "Name": name })).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "InvalidDocument"),
            "case {case:?}: {name} names no document here, got {status} {response}"
        );
    }
}

/// A closed enumeration a request sends EMPTY is outside its value set, so it is
/// refused rather than served as if the member had been omitted.
#[tokio::test]
async fn an_empty_enum_value_is_refused_rather_than_defaulted() {
    let router = router();
    for member in ["DocumentFormat", "DocumentType"] {
        let mut body = json!({ "Name": "EmptyEnumDocument", "Content": COMMAND_JSON });
        body[member] = json!("");
        let (status, response) = create(&router, &body).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {member} empty: an empty enum value is refused, got {status} {response}"
        );
    }
    // Omitting them is what gets the documented defaults.
    let created = create_ok(
        &router,
        "both members omitted",
        &json!({ "Name": "DefaultedEnumDocument", "Content": COMMAND_JSON }),
    )
    .await;
    assert_eq!(
        (
            created["DocumentFormat"].clone(),
            created["DocumentType"].clone()
        ),
        (json!("JSON"), json!("Command")),
        "an omitted format and type get the documented defaults, got {created}"
    );
}

/// The parameters a read reports carry the type, description and default the content
/// declared, with a non-scalar default rendered as the one string `DefaultValue` is.
#[tokio::test]
async fn a_read_reports_the_parameter_definitions_the_content_declared() {
    let router = router();
    create_ok(
        &router,
        "the parameter definitions",
        &json!({ "Name": "ParameterisedDocument", "Content": COMMAND_JSON }),
    )
    .await;
    let read = describe_ok(&router, "the parameter definitions", "ParameterisedDocument").await;
    assert_eq!(
        read["Parameters"],
        json!([
            {
                "Name": "Message",
                "Type": "String",
                "Description": "what to echo",
                "DefaultValue": "hello",
            },
            {
                "Name": "Commands",
                "Type": "StringList",
                "DefaultValue": "[\"echo one\",\"echo two\"]",
            },
        ]),
        "the reported parameters are the ones the content declares, got {read}"
    );
}
