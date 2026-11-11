// SPDX-License-Identifier: CC0-1.0

//! `ListDocuments` end to end: what an account holds in a Region, narrowed and paged.
//!
//! The rules under test:
//!
//! 1. A listing reports exactly the documents a read of this service resolves through the
//!    Region's endpoint — the ones a caller created there AND the ones Systems Manager
//!    publishes in every account — once each, by the identity of its DEFAULT version, and
//!    reports nothing of another Region.
//! 2. A filter narrows that listing by one attribute of a document: a `Name` prefix, an
//!    owner class, a platform, a document type, a target type or a tag. Filters are ANDed
//!    and the values of one filter ORed, and a key naming no attribute of a document is
//!    refused as `InvalidFilterKey`, as is an `Owner` value that names no owner class at
//!    all (a `ValidationException`) — a published class that nothing belongs to reports an
//!    empty page instead.
//! 3. A page holds at most `MaxResults` documents and the token hands back the name the
//!    next page resumes at, so paging walks every document once however small the page; a
//!    token this service did not issue is refused as `InvalidNextToken`.
//!
//! Every case goes through the front door a client uses — the shared harness in
//! `all/parameter_store.rs` ([`router`], [`call`], [`call_in_region`], [`error_code`])
//! dispatches `POST /` with `X-Amz-Target: AmazonSSM.<Operation>` through the
//! multi-service router. Nothing reaches into the state struct, so a listing that is not
//! computed from persisted state cannot pass: every listing follows the creates it must
//! report.
//!
//! The suites are tables. The rows span what each rule generalises over — document type,
//! document format, schema version, platform precondition, target type, tag set, owner
//! class, filter key, value count, how many filters are combined, page size and Region —
//! rather than the one document a recorded unit happened to create.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::parameter_store::{call, call_in_region, error_code, error_message, router, ACCOUNT, REGION};

/// The catalogue the service serves (`crate::state::PUBLISHED_DOCUMENTS`), so the name
/// lists below can be checked to cover every entry of it rather than a few.
const CATALOGUE: &[service_com_amazonaws_ssm_amazonssm::state::PublishedDocument] =
    service_com_amazonaws_ssm_amazonssm::state::PUBLISHED_DOCUMENTS;

/// The `Owner` the documents Systems Manager publishes report, which is the owner class
/// `ListDocuments`' `Owner` filter calls `Amazon`.
const AMAZON: &str = "Amazon";

/// A second Region, so a rule that holds per Region is seen to.
const OTHER_REGION: &str = "eu-central-1";

/// The `PlatformType` values the model's enum declares
/// (`smithy_query(query="shape", shape="PlatformType")`), which is what a document whose
/// body restricts it to no platform is compatible with.
const PLATFORM_TYPES: &[&str] = &["Linux", "MacOS", "Windows"];

/// The published `Automation` runbooks, in name order. Checked below to partition the
/// catalogue with [`PUBLISHED_COMMANDS`], so a new entry has to be given its place here.
const PUBLISHED_AUTOMATIONS: &[&str] = &[
    "AWS-ConfigureS3BucketPublicAccessBlock",
    "AWS-ConfigureS3BucketVersioning",
    "AWS-DisableS3BucketPublicReadWrite",
    "AWS-EnableS3BucketEncryption",
    "AWS-PublishSNSNotification",
];

/// The published `Command` documents, in name order.
const PUBLISHED_COMMANDS: &[&str] = &[
    "AWS-RunPowerShellScript",
    "AWS-RunShellScript",
    "AWS-UpdateSSMAgent",
];

/// A JSON `Command` document whose step precondition restricts it to Linux.
const COMMAND_LINUX_JSON: &str = r#"{
  "schemaVersion": "2.2",
  "description": "Echo a message on a Linux node",
  "mainSteps": [
    {
      "action": "aws:runShellScript",
      "name": "runShell",
      "precondition": { "StringEquals": ["platformType", "Linux"] },
      "inputs": { "runCommand": ["echo hi"] }
    }
  ]
}"#;

/// The same kind of document written in YAML, restricted to Windows.
const COMMAND_WINDOWS_YAML: &str = r#"schemaVersion: '2.2'
description: Echo a message on a Windows node
mainSteps:
  - name: runPowerShell
    action: 'aws:runPowerShellScript'
    precondition:
      StringEquals: [platformType, Windows]
    inputs:
      runCommand: ['Write-Host hi']
"#;

/// A YAML `Automation` runbook with no platform precondition at all.
const AUTOMATION_YAML: &str = r#"schemaVersion: '0.3'
description: Remediate a noncompliant bucket
assumeRole: '{{ AutomationAssumeRole }}'
parameters:
  AutomationAssumeRole:
    type: String
mainSteps:
  - name: putBucketLogging
    action: 'aws:executeAwsApi'
    inputs:
      Service: s3
      Api: PutBucketLogging
"#;

/// An access-request approval policy: a JSON body with no steps at all.
const APPROVAL_POLICY_JSON: &str = r#"{
  "schemaVersion": "1.0",
  "description": "who may approve a node access request"
}"#;

/// A Change Calendar entry: `TEXT` content, which carries no document tree.
const CALENDAR_TEXT: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nSUMMARY:change window\r\nEND:VCALENDAR";

/// One document a case creates, and what a listing must then report for it.
struct Created<'a> {
    case: &'a str,
    name: &'a str,
    document_type: Option<&'a str>,
    format: Option<&'a str>,
    content: &'a str,
    target_type: Option<&'a str>,
    display_name: Option<&'a str>,
    tags: &'a [(&'a str, &'a str)],
    expected_type: &'a str,
    expected_format: &'a str,
    /// The `schemaVersion` the body declares, or `None` for a body that declares none.
    schema_version: Option<&'a str>,
    /// The platforms the body's step preconditions restrict the document to.
    platforms: &'a [&'a str],
}

/// The documents every suite below creates: one row per document form the listing has to
/// report — the two platform preconditions, three document formats, four document types,
/// a target type, a display name and a tag set.
fn created_rows() -> Vec<Created<'static>> {
    vec![
        Created {
            case: "a JSON Command document with a Linux precondition, a target type and a tag",
            name: "EchoOnLinux",
            document_type: None,
            format: None,
            content: COMMAND_LINUX_JSON,
            target_type: Some("/AWS::EC2::Instance"),
            display_name: Some("Echo on Linux"),
            tags: &[("Environment", "Production")],
            expected_type: "Command",
            expected_format: "JSON",
            schema_version: Some("2.2"),
            platforms: &["Linux"],
        },
        Created {
            case: "a YAML Command document with a Windows precondition",
            name: "EchoOnWindows",
            document_type: Some("Command"),
            format: Some("YAML"),
            content: COMMAND_WINDOWS_YAML,
            target_type: None,
            display_name: None,
            tags: &[("Environment", "Development")],
            expected_type: "Command",
            expected_format: "YAML",
            schema_version: Some("2.2"),
            platforms: &["Windows"],
        },
        Created {
            case: "a YAML Automation runbook that runs anywhere",
            name: "RemediateBucket",
            document_type: Some("Automation"),
            format: Some("YAML"),
            content: AUTOMATION_YAML,
            target_type: Some("/"),
            display_name: None,
            tags: &[],
            expected_type: "Automation",
            expected_format: "YAML",
            schema_version: Some("0.3"),
            platforms: PLATFORM_TYPES,
        },
        Created {
            case: "a manual approval policy, a document type with no steps",
            name: "ApproveNodeAccess",
            document_type: Some("ManualApprovalPolicy"),
            format: Some("JSON"),
            content: APPROVAL_POLICY_JSON,
            target_type: None,
            display_name: None,
            tags: &[],
            expected_type: "ManualApprovalPolicy",
            expected_format: "JSON",
            schema_version: Some("1.0"),
            platforms: PLATFORM_TYPES,
        },
        Created {
            case: "TEXT content, which carries no document tree",
            name: "CalendarEntry",
            document_type: Some("ChangeCalendar"),
            format: Some("TEXT"),
            content: CALENDAR_TEXT,
            target_type: None,
            display_name: None,
            tags: &[],
            expected_type: "ChangeCalendar",
            expected_format: "TEXT",
            schema_version: None,
            platforms: &[],
        },
    ]
}

/// `CreateDocument` for one row, signed for `region`, asserting it succeeded.
async fn create_in(router: &MultiServiceRouter, region: &str, row: &Created<'_>) {
    let mut body = json!({ "Name": row.name, "Content": row.content });
    if let Some(document_type) = row.document_type {
        body["DocumentType"] = json!(document_type);
    }
    if let Some(format) = row.format {
        body["DocumentFormat"] = json!(format);
    }
    if let Some(target_type) = row.target_type {
        body["TargetType"] = json!(target_type);
    }
    if let Some(display_name) = row.display_name {
        body["DisplayName"] = json!(display_name);
    }
    if !row.tags.is_empty() {
        body["Tags"] = json!(row
            .tags
            .iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect::<Vec<Value>>());
    }
    let (status, response) = call_in_region(router, region, "CreateDocument", &body).await;
    assert_eq!(
        status, 200,
        "case {}: CreateDocument {} in {region} must succeed, got {status} {response}",
        row.case, row.name
    );
}

/// A router holding every row of [`created_rows`] in [`REGION`].
async fn router_with_documents() -> MultiServiceRouter {
    let router = router();
    for row in &created_rows() {
        create_in(&router, REGION, row).await;
    }
    router
}

/// `ListDocuments` with `body`, signed for `region`.
async fn list_in(router: &MultiServiceRouter, region: &str, body: &Value) -> (u16, Value) {
    call_in_region(router, region, "ListDocuments", body).await
}

/// `ListDocuments`, asserting it succeeded.
async fn list_ok(router: &MultiServiceRouter, case: &str, body: &Value) -> Value {
    let (status, response) = call(router, "ListDocuments", body).await;
    assert_eq!(
        status, 200,
        "case {case}: ListDocuments {body} must succeed, got {status} {response}"
    );
    response
}

/// The reported `DocumentIdentifiers`.
fn reported(response: &Value) -> Vec<Value> {
    response["DocumentIdentifiers"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The reported names, sorted — AWS documents no order for a listing, so every assertion
/// is about the SET a page or a walk reports.
fn names(response: &Value) -> Vec<String> {
    let mut reported: Vec<String> = reported(response)
        .iter()
        .map(|identifier| {
            identifier["Name"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    reported.sort();
    reported
}

/// The one reported identifier of `name`, asserting the listing reported it exactly once.
fn identifier(case: &str, response: &Value, name: &str) -> Value {
    let matching: Vec<Value> = reported(response)
        .into_iter()
        .filter(|identifier| identifier["Name"].as_str() == Some(name))
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "case {case}: {name} must be reported exactly once, got {:?} in {response}",
        matching.len()
    );
    matching.into_iter().next().expect("one identifier")
}

/// A reported string list as owned strings.
fn string_list(identifier: &Value, member: &str) -> Vec<String> {
    identifier[member]
        .as_array()
        .map(|values| {
            values
                .iter()
                .map(|value| value.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The reported tags as sorted `(key, value)` pairs.
fn tag_pairs(identifier: &Value) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = identifier["Tags"]
        .as_array()
        .map(|tags| {
            tags.iter()
                .map(|tag| {
                    (
                        tag["Key"].as_str().unwrap_or_default().to_string(),
                        tag["Value"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    pairs.sort();
    pairs
}

/// Every name the account holds in [`REGION`] once the rows are created: the created
/// documents beside the published catalogue.
fn every_name() -> Vec<String> {
    let mut all: Vec<String> = created_rows()
        .iter()
        .map(|row| row.name.to_string())
        .chain(CATALOGUE.iter().map(|entry| entry.name.to_string()))
        .collect();
    all.sort();
    all
}

/// `names` sorted, as owned strings — the expected side of a set assertion.
fn sorted(names: &[&str]) -> Vec<String> {
    let mut sorted: Vec<String> = names.iter().map(|name| name.to_string()).collect();
    sorted.sort();
    sorted
}

/// The catalogue's own names, sorted.
fn catalogue_names() -> Vec<String> {
    let mut names: Vec<String> = CATALOGUE.iter().map(|entry| entry.name.to_string()).collect();
    names.sort();
    names
}

/// A listing with no filter reports every document the Region holds — the creates beside
/// the catalogue — once each, and each one by its stored identity.
#[tokio::test]
async fn a_listing_reports_every_document_the_region_holds() {
    let router = router_with_documents().await;
    let response = list_ok(&router, "a listing with no filter", &json!({})).await;

    assert_eq!(
        names(&response),
        every_name(),
        "a listing must report the created documents beside the published catalogue, \
         got {response}"
    );
    assert!(
        response["NextToken"]
            .as_str()
            .is_none_or(|token| token.is_empty()),
        "a listing that reported everything hands back no resume token, got {response}"
    );

    for row in &created_rows() {
        let identifier = identifier(row.case, &response, row.name);
        assert_eq!(
            identifier["Owner"].as_str(),
            Some(ACCOUNT),
            "case {}: a document this account created is owned by it, got {identifier}",
            row.case
        );
        assert_eq!(
            identifier["DocumentType"].as_str(),
            Some(row.expected_type),
            "case {}: expected DocumentType {}, got {identifier}",
            row.case,
            row.expected_type
        );
        assert_eq!(
            identifier["DocumentFormat"].as_str(),
            Some(row.expected_format),
            "case {}: expected DocumentFormat {}, got {identifier}",
            row.case,
            row.expected_format
        );
        assert_eq!(
            identifier["SchemaVersion"].as_str(),
            row.schema_version,
            "case {}: expected SchemaVersion {:?}, got {identifier}",
            row.case,
            row.schema_version
        );
        assert_eq!(
            string_list(&identifier, "PlatformTypes"),
            row.platforms,
            "case {}: expected PlatformTypes {:?}, got {identifier}",
            row.case,
            row.platforms
        );
        assert_eq!(
            identifier["DocumentVersion"].as_str(),
            Some("1"),
            "case {}: a created document is reported at its default version, got {identifier}",
            row.case
        );
        assert_eq!(
            identifier["TargetType"].as_str(),
            row.target_type,
            "case {}: expected TargetType {:?}, got {identifier}",
            row.case,
            row.target_type
        );
        assert_eq!(
            identifier["DisplayName"].as_str(),
            row.display_name,
            "case {}: expected DisplayName {:?}, got {identifier}",
            row.case,
            row.display_name
        );
        let expected_tags: Vec<(String, String)> = {
            let mut pairs: Vec<(String, String)> = row
                .tags
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect();
            pairs.sort();
            pairs
        };
        assert_eq!(
            tag_pairs(&identifier),
            expected_tags,
            "case {}: expected the tags the create supplied, got {identifier}",
            row.case
        );
        assert!(
            identifier["CreatedDate"].is_number(),
            "case {}: a listing reports when the version was created, got {identifier}",
            row.case
        );
    }

    // The catalogue is reported by the owner it belongs to and at the only version it
    // holds, whatever the account created.
    for entry in CATALOGUE {
        let identifier = identifier("a published document", &response, entry.name);
        assert_eq!(
            identifier["Owner"].as_str(),
            Some(AMAZON),
            "case a published document: {} is owned by {AMAZON}, got {identifier}",
            entry.name
        );
        assert_eq!(
            identifier["DocumentType"].as_str(),
            Some(entry.document_type),
            "case a published document: expected DocumentType {}, got {identifier}",
            entry.document_type
        );
        assert_eq!(
            identifier["DocumentVersion"].as_str(),
            Some("1"),
            "case a published document: {} holds one version, got {identifier}",
            entry.name
        );
    }

    // A listing is a REPORT: a second one reports what the first did.
    let again = list_ok(&router, "a repeated listing", &json!({})).await;
    assert_eq!(
        reported(&again),
        reported(&response),
        "a listing must not change what a later listing reports, got {again}"
    );
}

/// What a listing reports of a document agrees with what a read of that document reports:
/// both resolve the same record at the same default version.
#[tokio::test]
async fn a_listing_agrees_with_a_read_of_each_document() {
    let router = router_with_documents().await;
    let response = list_ok(&router, "a listing to cross-check", &json!({})).await;

    for name in every_name() {
        let identifier = identifier("cross-check", &response, &name);
        let (status, read) = call(&router, "DescribeDocument", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case cross-check {name}: DescribeDocument must succeed, got {status} {read}"
        );
        let document = read["Document"].clone();
        for member in [
            "Name",
            "Owner",
            "DocumentType",
            "DocumentFormat",
            "SchemaVersion",
            "DocumentVersion",
            "TargetType",
            "DisplayName",
        ] {
            assert_eq!(
                identifier[member], document[member],
                "case cross-check {name}: the listing's {member} must be the read's, got \
                 {identifier} and {document}"
            );
        }
        assert_eq!(
            string_list(&identifier, "PlatformTypes"),
            string_list(&document, "PlatformTypes"),
            "case cross-check {name}: the listing's PlatformTypes must be the read's, got \
             {identifier} and {document}"
        );
    }
}

/// A document is reported at the DEFAULT version, which is where an update leaves it and
/// where a default-version switch moves it.
#[tokio::test]
async fn a_document_is_reported_at_its_default_version() {
    let router = router();
    let name = "VersionedDocument";
    let (status, created) = call(
        &router,
        "CreateDocument",
        &json!({
            "Name": name,
            "Content": COMMAND_LINUX_JSON,
            "DisplayName": "first display name",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the document must be created, got {status} {created}"
    );

    // Each row: how many versions the document holds, which one is the default, and what a
    // listing must then report for it.
    let (status, updated) = call(
        &router,
        "UpdateDocument",
        &json!({
            "Name": name,
            "Content": COMMAND_WINDOWS_YAML,
            "DocumentFormat": "YAML",
            "DisplayName": "second display name",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the document must take a second version, got {status} {updated}"
    );

    let cases: &[(&str, Option<&str>, &str, &str, &str)] = &[
        (
            "an update appends a version without moving the default",
            None,
            "1",
            "JSON",
            "first display name",
        ),
        (
            "the default moves to the appended version",
            Some("2"),
            "2",
            "YAML",
            "second display name",
        ),
        (
            "and back to the first",
            Some("1"),
            "1",
            "JSON",
            "first display name",
        ),
    ];

    for (case, switch_to, expected_version, expected_format, expected_display) in cases {
        if let Some(version) = switch_to {
            let (status, switched) = call(
                &router,
                "UpdateDocumentDefaultVersion",
                &json!({ "Name": name, "DocumentVersion": version }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case}: the default must move to {version}, got {status} {switched}"
            );
        }
        let response = list_ok(&router, case, &json!({ "Filters": [{ "Key": "Owner", "Values": ["Self"] }] })).await;
        let identifier = identifier(case, &response, name);
        assert_eq!(
            identifier["DocumentVersion"].as_str(),
            Some(*expected_version),
            "case {case}: expected DocumentVersion {expected_version}, got {identifier}"
        );
        assert_eq!(
            identifier["DocumentFormat"].as_str(),
            Some(*expected_format),
            "case {case}: expected the default version's DocumentFormat, got {identifier}"
        );
        assert_eq!(
            identifier["DisplayName"].as_str(),
            Some(*expected_display),
            "case {case}: expected the default version's DisplayName, got {identifier}"
        );
    }
}

/// Every filter form narrows the listing by the attribute its key names: values of one
/// filter are ORed, and two filters are ANDed.
#[tokio::test]
async fn filters_narrow_the_listing_by_one_attribute() {
    // The name lists below have to partition the catalogue, so a new published document
    // cannot silently fall out of the expectations.
    for entry in CATALOGUE {
        assert!(
            PUBLISHED_AUTOMATIONS.contains(&entry.name) || PUBLISHED_COMMANDS.contains(&entry.name),
            "the expectations must carry published document {}",
            entry.name
        );
    }
    assert_eq!(
        PUBLISHED_AUTOMATIONS.len() + PUBLISHED_COMMANDS.len(),
        CATALOGUE.len(),
        "the expectations must name only documents the service publishes"
    );

    let router = router_with_documents().await;
    let created_names: Vec<&str> = vec![
        "ApproveNodeAccess",
        "CalendarEntry",
        "EchoOnLinux",
        "EchoOnWindows",
        "RemediateBucket",
    ];
    let every_command: Vec<&str> = PUBLISHED_COMMANDS
        .iter()
        .copied()
        .chain(["EchoOnLinux", "EchoOnWindows"])
        .collect();
    // A document is compatible with Windows when its body's preconditions do not restrict
    // it to another platform: the two Windows documents, everything that restricts itself
    // to no platform, and nothing whose steps are Linux-only or that carries no tree.
    let windows: Vec<&str> = PUBLISHED_AUTOMATIONS
        .iter()
        .copied()
        .chain([
            "AWS-RunPowerShellScript",
            "AWS-UpdateSSMAgent",
            "ApproveNodeAccess",
            "EchoOnWindows",
            "RemediateBucket",
        ])
        .collect();

    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "Owner=Self names what this account created",
            json!({ "Filters": [{ "Key": "Owner", "Values": ["Self"] }] }),
            sorted(&created_names),
        ),
        (
            "Owner=Private names the same documents, which are shared with nobody",
            json!({ "Filters": [{ "Key": "Owner", "Values": ["Private"] }] }),
            sorted(&created_names),
        ),
        (
            "Owner=Amazon names the published catalogue",
            json!({ "Filters": [{ "Key": "Owner", "Values": ["Amazon"] }] }),
            catalogue_names(),
        ),
        (
            "Owner=Public names nothing this account holds",
            json!({ "Filters": [{ "Key": "Owner", "Values": ["Public"] }] }),
            Vec::new(),
        ),
        (
            "Owner=ThirdParty names nothing this account holds",
            json!({ "Filters": [{ "Key": "Owner", "Values": ["ThirdParty"] }] }),
            Vec::new(),
        ),
        (
            "a Name value is a prefix",
            json!({ "Filters": [{ "Key": "Name", "Values": ["Echo"] }] }),
            sorted(&["EchoOnLinux", "EchoOnWindows"]),
        ),
        (
            "a longer prefix narrows further",
            json!({ "Filters": [{ "Key": "Name", "Values": ["EchoOnL"] }] }),
            sorted(&["EchoOnLinux"]),
        ),
        (
            "a prefix of the published catalogue",
            json!({ "Filters": [{ "Key": "Name", "Values": ["AWS-Run"] }] }),
            sorted(&["AWS-RunPowerShellScript", "AWS-RunShellScript"]),
        ),
        (
            "two prefixes are ORed",
            json!({ "Filters": [{ "Key": "Name", "Values": ["Remediate", "Calendar"] }] }),
            sorted(&["CalendarEntry", "RemediateBucket"]),
        ),
        (
            "a prefix nothing carries names nothing",
            json!({ "Filters": [{ "Key": "Name", "Values": ["NoSuchDocument"] }] }),
            Vec::new(),
        ),
        (
            "DocumentType spans the created and the published documents of that type",
            json!({ "Filters": [{ "Key": "DocumentType", "Values": ["Command"] }] }),
            sorted(&every_command),
        ),
        (
            "two document types are ORed",
            json!({ "Filters": [{ "Key": "DocumentType", "Values": ["ChangeCalendar", "ManualApprovalPolicy"] }] }),
            sorted(&["ApproveNodeAccess", "CalendarEntry"]),
        ),
        (
            "a document type nothing holds names nothing",
            json!({ "Filters": [{ "Key": "DocumentType", "Values": ["Session"] }] }),
            Vec::new(),
        ),
        (
            "PlatformTypes names every document compatible with that platform",
            json!({ "Filters": [{ "Key": "PlatformTypes", "Values": ["Windows"] }] }),
            sorted(&windows),
        ),
        (
            "a platform a Linux-only document is not compatible with",
            json!({
                "Filters": [
                    { "Key": "PlatformTypes", "Values": ["Linux"] },
                    { "Key": "Owner", "Values": ["Self"] },
                ]
            }),
            sorted(&[
                "ApproveNodeAccess",
                "EchoOnLinux",
                "RemediateBucket",
            ]),
        ),
        (
            "TargetType matches the whole value",
            json!({ "Filters": [{ "Key": "TargetType", "Values": ["/AWS::EC2::Instance"] }] }),
            sorted(&["EchoOnLinux"]),
        ),
        (
            "the target type that names every resource type",
            json!({ "Filters": [{ "Key": "TargetType", "Values": ["/"] }] }),
            sorted(&["RemediateBucket"]),
        ),
        (
            "a tag key and value",
            json!({ "Filters": [{ "Key": "tag:Environment", "Values": ["Production"] }] }),
            sorted(&["EchoOnLinux"]),
        ),
        (
            "one tag key with two values",
            json!({ "Filters": [{ "Key": "tag:Environment", "Values": ["Production", "Development"] }] }),
            sorted(&["EchoOnLinux", "EchoOnWindows"]),
        ),
        (
            "a tag value nothing carries",
            json!({ "Filters": [{ "Key": "tag:Environment", "Values": ["Staging"] }] }),
            Vec::new(),
        ),
        (
            "a tag key nothing carries",
            json!({ "Filters": [{ "Key": "tag:Project", "Values": ["Production"] }] }),
            Vec::new(),
        ),
        (
            "two keys are ANDed",
            json!({
                "Filters": [
                    { "Key": "Owner", "Values": ["Self"] },
                    { "Key": "DocumentType", "Values": ["Command"] },
                ]
            }),
            sorted(&["EchoOnLinux", "EchoOnWindows"]),
        ),
        (
            "three keys are ANDed",
            json!({
                "Filters": [
                    { "Key": "Owner", "Values": ["Self"] },
                    { "Key": "Name", "Values": ["Echo"] },
                    { "Key": "tag:Environment", "Values": ["Production"] },
                ]
            }),
            sorted(&["EchoOnLinux"]),
        ),
        (
            "an AND nothing satisfies",
            json!({
                "Filters": [
                    { "Key": "Owner", "Values": ["Amazon"] },
                    { "Key": "Name", "Values": ["Echo"] },
                ]
            }),
            Vec::new(),
        ),
        (
            "the deprecated filter list carries one value per entry",
            json!({ "DocumentFilterList": [{ "key": "Owner", "value": "Self" }] }),
            sorted(&created_names),
        ),
        (
            "the deprecated filter list ANDs its entries",
            json!({
                "DocumentFilterList": [
                    { "key": "Owner", "value": "Self" },
                    { "key": "DocumentType", "value": "Automation" },
                ]
            }),
            sorted(&["RemediateBucket"]),
        ),
        (
            "both filter members narrow one listing",
            json!({
                "DocumentFilterList": [{ "key": "DocumentType", "value": "Command" }],
                "Filters": [{ "Key": "Owner", "Values": ["Amazon"] }],
            }),
            sorted(PUBLISHED_COMMANDS),
        ),
    ];

    for (case, body, expected) in cases {
        let response = list_ok(&router, case, &body).await;
        assert_eq!(
            names(&response),
            expected,
            "case {case}: {body} must report {expected:?}, got {response}"
        );
    }
}

/// A page holds at most `MaxResults` documents and the token resumes the walk, so paging
/// reports every document once however small the page.
#[tokio::test]
async fn a_listing_pages_by_document_name() {
    let router = router_with_documents().await;
    let every = every_name();
    // Page sizes that divide the holding and ones that do not, each published bound of the
    // modelled range (1..50), and one that holds everything.
    for page_size in [1usize, 2, 3, 5, 7, every.len(), 50] {
        let case = format!("MaxResults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..=every.len() {
            let mut body = json!({ "MaxResults": page_size });
            if let Some(token) = &token {
                body["NextToken"] = json!(token);
            }
            let response = list_ok(&router, &case, &body).await;
            let page = names(&response);
            assert!(
                page.len() <= page_size,
                "case {case}: a page carries at most {page_size} documents, got {} in {response}",
                page.len()
            );
            walked.extend(page);
            token = response["NextToken"]
                .as_str()
                .filter(|token| !token.is_empty())
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert!(
            token.is_none(),
            "case {case}: the walk must end, got a token after {} documents",
            walked.len()
        );
        walked.sort();
        assert_eq!(
            walked, every,
            "case {case}: the walk must report every document exactly once, got {walked:?}"
        );
    }
}

/// A filtered listing pages the same way: the filter narrows the walk and the token
/// resumes it.
#[tokio::test]
async fn a_filtered_listing_pages_too() {
    let router = router_with_documents().await;
    let expected = sorted(&[
        "ApproveNodeAccess",
        "CalendarEntry",
        "EchoOnLinux",
        "EchoOnWindows",
        "RemediateBucket",
    ]);
    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..=expected.len() {
        let mut body = json!({
            "MaxResults": 2,
            "Filters": [{ "Key": "Owner", "Values": ["Self"] }],
        });
        if let Some(token) = &token {
            body["NextToken"] = json!(token);
        }
        let response = list_ok(&router, "a filtered walk", &body).await;
        walked.extend(names(&response));
        token = response["NextToken"]
            .as_str()
            .filter(|token| !token.is_empty())
            .map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    walked.sort();
    assert_eq!(
        walked, expected,
        "a filtered walk must report every document the filter admits once, got {walked:?}"
    );
}

/// A document is reached through the endpoint it was created against: a listing of one
/// Region reports nothing another Region holds, and the catalogue in both.
#[tokio::test]
async fn a_listing_reports_only_the_region_it_is_signed_for() {
    let router = router();
    let rows = created_rows();
    let here = &rows[0];
    let there = &rows[1];
    create_in(&router, REGION, here).await;
    create_in(&router, OTHER_REGION, there).await;

    for (case, region, created) in [
        ("the Region the first create was signed for", REGION, here.name),
        (
            "the Region the second create was signed for",
            OTHER_REGION,
            there.name,
        ),
    ] {
        let (status, response) = list_in(&router, region, &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case}: ListDocuments in {region} must succeed, got {status} {response}"
        );
        let mut expected = catalogue_names();
        expected.push(created.to_string());
        expected.sort();
        assert_eq!(
            names(&response),
            expected,
            "case {case}: {region} holds only its own document beside the catalogue, got \
             {response}"
        );
    }
}

/// What a listing refuses, and with which modelled error.
#[tokio::test]
async fn a_listing_refuses_a_request_it_cannot_serve() {
    let router = router_with_documents().await;
    // (case, request, the error code the refusal carries)
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a filter key naming no attribute of a document",
            json!({ "Filters": [{ "Key": "DocumentName", "Values": ["Echo"] }] }),
            "InvalidFilterKey",
        ),
        (
            "a filter key differing from a valid one only in case",
            json!({ "Filters": [{ "Key": "owner", "Values": ["Self"] }] }),
            "InvalidFilterKey",
        ),
        (
            "the tag prefix with no tag key after it",
            json!({ "Filters": [{ "Key": "tag:", "Values": ["Production"] }] }),
            "InvalidFilterKey",
        ),
        (
            "a filter naming no key at all",
            json!({ "Filters": [{ "Values": ["Self"] }] }),
            "InvalidFilterKey",
        ),
        (
            "a filter carrying no value to search for",
            json!({ "Filters": [{ "Key": "Owner", "Values": [] }] }),
            "ValidationException",
        ),
        (
            "more than one owner class",
            json!({ "Filters": [{ "Key": "Owner", "Values": ["Self", "Amazon"] }] }),
            "ValidationException",
        ),
        (
            "an Owner value that names no owner class",
            json!({ "Filters": [{ "Key": "Owner", "Values": ["Nobody"] }] }),
            "ValidationException",
        ),
        (
            "an Owner value differing from a class only in case",
            json!({ "Filters": [{ "Key": "Owner", "Values": ["self"] }] }),
            "ValidationException",
        ),
        (
            "an account id, which is not an owner class either",
            json!({ "Filters": [{ "Key": "Owner", "Values": [ACCOUNT] }] }),
            "ValidationException",
        ),
        (
            "the deprecated filter list naming no owner class",
            json!({ "DocumentFilterList": [{ "key": "Owner", "value": "Nobody" }] }),
            "ValidationException",
        ),
        (
            "more than one tag key",
            json!({
                "Filters": [
                    { "Key": "tag:Environment", "Values": ["Production"] },
                    { "Key": "tag:Project", "Values": ["Apollo"] },
                ]
            }),
            "ValidationException",
        ),
        (
            "MaxResults one below the modelled minimum",
            json!({ "MaxResults": 0 }),
            "ValidationException",
        ),
        (
            "MaxResults one above the modelled maximum",
            json!({ "MaxResults": 51 }),
            "ValidationException",
        ),
        (
            "a NextToken shorter than any document name",
            json!({ "NextToken": "ab" }),
            "InvalidNextToken",
        ),
        (
            "a NextToken carrying a character no document name may",
            json!({ "NextToken": "Echo On Linux" }),
            "InvalidNextToken",
        ),
        (
            "a NextToken longer than any document name",
            json!({ "NextToken": "E".repeat(129) }),
            "InvalidNextToken",
        ),
    ];

    for (case, body, expected) in cases {
        let (status, response) = call(&router, "ListDocuments", &body).await;
        assert_eq!(
            status, 400,
            "case {case}: {body} must be refused with HTTP 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            expected,
            "case {case}: {body} must be refused with {expected}, got {response}"
        );
        assert!(
            !error_message(&response).is_empty(),
            "case {case}: the refusal must say why, got {response}"
        );
    }

    // The published bounds of `MaxResults` themselves are served.
    for page_size in [1, 50] {
        let response = list_ok(
            &router,
            &format!("MaxResults={page_size} is inside the modelled range"),
            &json!({ "MaxResults": page_size }),
        )
        .await;
        assert!(
            !reported(&response).is_empty(),
            "case MaxResults={page_size}: a page inside the modelled range is served, got \
             {response}"
        );
    }

    // A token this service DID issue resumes the walk rather than being refused.
    let first = list_ok(&router, "the first page", &json!({ "MaxResults": 1 })).await;
    let token = first["NextToken"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !token.is_empty(),
        "a page that left documents over hands back a token, got {first}"
    );
    let second = list_ok(
        &router,
        "the page the token resumes",
        &json!({ "MaxResults": 1, "NextToken": token }),
    )
    .await;
    assert_eq!(
        reported(&second).len(),
        1,
        "the resumed page reports the next document, got {second}"
    );
    assert_ne!(
        names(&second),
        names(&first),
        "the resumed page continues past the first, got {second}"
    );
}

/// An account that created nothing still holds the catalogue, and holds it the same way in
/// every Region.
#[tokio::test]
async fn a_fresh_account_holds_the_published_catalogue_only() {
    let router = router();
    for region in [REGION, OTHER_REGION] {
        let (status, response) = list_in(&router, region, &json!({})).await;
        assert_eq!(
            status, 200,
            "case {region}: ListDocuments must succeed on a fresh account, got {status} \
             {response}"
        );
        assert_eq!(
            names(&response),
            catalogue_names(),
            "case {region}: a fresh account holds the published catalogue only, got {response}"
        );
        let (status, own) = list_in(
            &router,
            region,
            &json!({ "Filters": [{ "Key": "Owner", "Values": ["Self"] }] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region}: the owned listing must succeed, got {status} {own}"
        );
        assert_eq!(
            names(&own),
            Vec::<String>::new(),
            "case {region}: an account that created nothing owns nothing, got {own}"
        );
    }
}
