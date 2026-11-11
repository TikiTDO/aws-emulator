// SPDX-License-Identifier: CC0-1.0

//! State Manager associations end to end: `CreateAssociation` persisting the association
//! an account wants applied to its managed nodes, and `DescribeAssociation` reporting back
//! the association — and the version of it — that the create persisted.
//!
//! Every case goes through the front door a client uses — `POST /` with
//! `X-Amz-Target: AmazonSSM.<Operation>` and an `awsJson1_1` body, dispatched by the
//! multi-service router (`OPERATIONS.md`, "Wire protocol").
//!
//! The suites are tables. Each row is one input along the axis the rule under test
//! generalises over — document, document spelling, document version, targeting form,
//! schedule form, schedule interval, parameter set, Region, version selector, tag set —
//! rather than the single create the recorded units happened to make.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_ssm_amazonssm::state::{associations, ComAmazonawsSsmAmazonssmState};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request signs for unless a case says otherwise. Deliberately not the
/// emulator's default, so an association served here proves the Region came from the
/// request's SigV4 credential scope.
pub(crate) const REGION: &str = "us-west-2";

/// A second Region, to pin that an association is reachable only through the Region it was
/// created in.
pub(crate) const OTHER_REGION: &str = "us-east-1";

/// A third Region, to pin that the store's own total guard spans every Region while the
/// documented association limit is measured per Region.
const THIRD_REGION: &str = "eu-west-1";

/// The `X-Amz-Target` prefix from the crate's `OPERATIONS.md` wire-protocol section.
const TARGET_PREFIX: &str = "AmazonSSM";

/// A command document Systems Manager publishes whose every parameter carries a default,
/// so a create that supplies none satisfies it (`state::PUBLISHED_DOCUMENTS`).
pub(crate) const AGENT: &str = "AWS-UpdateSSMAgent";

/// A command document Systems Manager publishes that declares one parameter with no
/// default, so a create has to supply a value for it (same catalogue).
pub(crate) const SHELL: &str = "AWS-RunShellScript";

/// An automation document Systems Manager publishes, to span document type (same
/// catalogue).
const AUTOMATION: &str = "AWS-EnableS3BucketEncryption";

/// A node id of the form the modelled `InstanceId` pattern admits (`i-` and 17
/// characters), named by the deprecated `InstanceId` member and by a target.
pub(crate) const NODE: &str = "i-0123456789abcdef0";

/// A second node id, of the shorter form the same pattern admits.
pub(crate) const OTHER_NODE: &str = "i-0abc1234";

/// A router with just this service mounted, and the engine it was mounted on — the handle
/// a case reads the store through.
pub(crate) fn service() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are not what these tests are about; the router still reads the SigV4
    // credential scope below, which is where the Region comes from.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_ssm_amazonssm::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// Call `operation` signed for `region`, returning `(status, body)`.
pub(crate) async fn call_in_region(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20240101/{region}/ssm/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a valid test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // The error shape travels in `__type`; fold in the header the framework also sets so
    // one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// Call `operation` signed for [`REGION`].
pub(crate) async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    call_in_region(router, REGION, operation, body).await
}

/// `CreateAssociation` with `body`, signed for [`REGION`].
pub(crate) async fn create(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "CreateAssociation", body).await
}

/// The `AssociationDescription` of a create that must have succeeded.
pub(crate) async fn created(router: &MultiServiceRouter, case: &str, body: &Value) -> Value {
    let (status, parsed) = create(router, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {parsed:?}"
    );
    parsed["AssociationDescription"].clone()
}

/// `DescribeAssociation` with `body`, signed for `region`.
async fn describe_in_region(
    router: &MultiServiceRouter,
    region: &str,
    body: &Value,
) -> (u16, Value) {
    call_in_region(router, region, "DescribeAssociation", body).await
}

/// The `AssociationDescription` a read of `body` must report.
pub(crate) async fn described(router: &MultiServiceRouter, case: &str, body: &Value) -> Value {
    let (status, parsed) = describe_in_region(router, REGION, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeAssociation, got {status} with \
         {parsed:?}"
    );
    parsed["AssociationDescription"].clone()
}

/// The wire error code of a failure body (`__type`, without its namespace).
pub(crate) fn error_code(body: &Value) -> String {
    let rendered = body["__type"].as_str().unwrap_or("<no __type>");
    rendered
        .split('#')
        .next_back()
        .unwrap_or(rendered)
        .to_string()
}

/// The refusal message of a failure body.
fn error_message(body: &Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or("<no message>")
        .to_string()
}

/// A string member of a response body.
fn text(body: &Value, member: &str) -> String {
    body[member].as_str().unwrap_or("<unset>").to_string()
}

/// The description with its `Overview` taken out.
///
/// The overview carries the association's own progress, which the clock moves off
/// `Creating` once the create has settled ([`associations::ASSOCIATION_CREATION_DELAY`]), so
/// two reads taken at different instants are compared over everything else and the overview
/// is asserted for what holds at EVERY instant ([`assert_progress_only`]).
pub(crate) fn without_overview(description: &Value) -> Value {
    let mut stripped = description.clone();
    if let Some(members) = stripped.as_object_mut() {
        members.remove("Overview");
    }
    stripped
}

/// An association nothing has run reports its own progress and no execution: `Pending`, the
/// creating detail only while its create is still settling, and no aggregated count or
/// execution date at any instant.
fn assert_progress_only(description: &Value, case: &str) {
    assert_eq!(
        description["Overview"]["Status"],
        json!("Pending"),
        "case {case:?}: an association nothing has run is Pending, got {description:?}"
    );
    let detailed = description["Overview"]["DetailedStatus"].clone();
    assert!(
        detailed.is_null() || detailed == json!("Creating"),
        "case {case:?}: expected the creating detail while the create settles or none after \
         it, got {detailed:?}"
    );
    assert!(
        description["Overview"]["AssociationStatusAggregatedCount"].is_null()
            && description["LastExecutionDate"].is_null()
            && description["LastSuccessfulExecutionDate"].is_null(),
        "case {case:?}: an association nothing has run reports no execution, got \
         {description:?}"
    );
}

/// How many associations the store holds, to pin that a refused create persisted nothing.
pub(crate) fn stored_associations(engine: &Arc<engine::InMemoryEngine>) -> usize {
    engine
        .state::<ComAmazonawsSsmAmazonssmState>()
        .lock()
        .associations
        .len()
}

/// Whether `id` is spelled the way the modelled `AssociationId` pattern requires:
/// `^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`.
fn is_association_id(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(wanted, group)| {
                group.len() == *wanted && group.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
}

/// One create and what the association it persists has to report back.
struct Row {
    case: &'static str,
    request: Value,
    /// The document name the association reports, which is the document's NAME whatever
    /// spelling the create named it by.
    name: &'static str,
    /// The members the association echoes, as `(member, value)` pairs.
    echoed: Vec<(&'static str, Value)>,
    /// The members the association reports NOTHING for, because nothing set them and no
    /// execution has produced one.
    unset: Vec<&'static str>,
}

/// The table of creates: one row per form a create can take — the document and its type,
/// the targeting form (a node named the deprecated way, node-id targets, tag targets,
/// target maps, no targets at all), the schedule form, the parameter set and the optional
/// members a create may carry.
fn creation_rows() -> Vec<Row> {
    vec![
        Row {
            case: "a command document with no targets and no schedule",
            request: json!({"Name": AGENT}),
            name: AGENT,
            echoed: vec![("AssociationVersion", json!("1"))],
            unset: vec![
                "Targets",
                "ScheduleExpression",
                "AssociationName",
                "InstanceId",
                "LastExecutionDate",
                "LastSuccessfulExecutionDate",
                "Status",
            ],
        },
        Row {
            case: "node-id targets, a cron schedule and an association name",
            request: json!({
                "Name": AGENT,
                "Targets": [{"Key": "InstanceIds", "Values": [NODE, OTHER_NODE]}],
                "ScheduleExpression": "cron(0 2 ? * SUN *)",
                "AssociationName": "nightly-agent-update",
                "ApplyOnlyAtCronInterval": true,
                "ComplianceSeverity": "MEDIUM",
                "SyncCompliance": "MANUAL",
                "MaxConcurrency": "10%",
                "MaxErrors": "2",
            }),
            name: AGENT,
            echoed: vec![
                ("AssociationVersion", json!("1")),
                (
                    "Targets",
                    json!([{"Key": "InstanceIds", "Values": [NODE, OTHER_NODE]}]),
                ),
                ("ScheduleExpression", json!("cron(0 2 ? * SUN *)")),
                ("AssociationName", json!("nightly-agent-update")),
                ("ApplyOnlyAtCronInterval", json!(true)),
                ("ComplianceSeverity", json!("MEDIUM")),
                ("SyncCompliance", json!("MANUAL")),
                ("MaxConcurrency", json!("10%")),
                ("MaxErrors", json!("2")),
            ],
            unset: vec!["InstanceId", "Status", "LastExecutionDate"],
        },
        Row {
            case: "tag targets, a rate schedule and parameters the document declares",
            request: json!({
                "Name": SHELL,
                "Targets": [{"Key": "tag:Env", "Values": ["prod"]}],
                "ScheduleExpression": "rate(12 hours)",
                "Parameters": {"commands": ["echo one", "echo two"]},
                "CalendarNames": ["AWS-Calendar"],
                "Duration": 3,
            }),
            name: SHELL,
            echoed: vec![
                ("AssociationVersion", json!("1")),
                ("Targets", json!([{"Key": "tag:Env", "Values": ["prod"]}])),
                ("ScheduleExpression", json!("rate(12 hours)")),
                ("Parameters", json!({"commands": ["echo one", "echo two"]})),
                ("CalendarNames", json!(["AWS-Calendar"])),
                ("Duration", json!(3)),
            ],
            unset: vec!["InstanceId", "AssociationName", "MaxConcurrency"],
        },
        Row {
            case: "the deprecated InstanceId member names the node",
            request: json!({"Name": AGENT, "InstanceId": NODE}),
            name: AGENT,
            echoed: vec![
                ("AssociationVersion", json!("1")),
                ("InstanceId", json!(NODE)),
            ],
            unset: vec!["Targets", "ScheduleExpression"],
        },
        Row {
            case: "an automation document with target maps and a document version",
            request: json!({
                "Name": AUTOMATION,
                "DocumentVersion": "1",
                "Parameters": {"BucketName": ["logs-bucket"]},
                "TargetMaps": [{"BucketName": ["logs-bucket"]}],
                "ScheduleOffset": 3,
                "ApplyOnlyAtCronInterval": true,
                "AutomationTargetParameterName": "BucketName",
            }),
            name: AUTOMATION,
            echoed: vec![
                ("AssociationVersion", json!("1")),
                ("DocumentVersion", json!("1")),
                ("Parameters", json!({"BucketName": ["logs-bucket"]})),
                ("TargetMaps", json!([{"BucketName": ["logs-bucket"]}])),
                ("ScheduleOffset", json!(3)),
                ("AutomationTargetParameterName", json!("BucketName")),
            ],
            unset: vec!["Targets", "InstanceId"],
        },
        Row {
            case: "the document named by its ARN, with an output location",
            request: json!({
                "Name": format!(
                    "arn:aws:ssm:{REGION}:{}:document/{AGENT}",
                    engine::emulator_account_id()
                ),
                "OutputLocation": {"S3Location": {
                    "OutputS3BucketName": "association-output",
                    "OutputS3KeyPrefix": "state-manager",
                    "OutputS3Region": REGION,
                }},
            }),
            // The ARN spelling names the same document, and the association reports the
            // document's NAME.
            name: AGENT,
            echoed: vec![
                ("AssociationVersion", json!("1")),
                (
                    "OutputLocation",
                    json!({"S3Location": {
                        "OutputS3BucketName": "association-output",
                        "OutputS3KeyPrefix": "state-manager",
                        "OutputS3Region": REGION,
                    }}),
                ),
            ],
            unset: vec!["Targets", "InstanceId"],
        },
    ]
}

/// A create persists the association, answers with what it persisted, and the reader
/// reports that same association back — whatever form the create took.
#[tokio::test]
async fn a_created_association_is_what_the_reader_reports_back() {
    for row in creation_rows() {
        let (router, engine) = service();
        let created = created(&router, row.case, &row.request).await;

        let association_id = text(&created, "AssociationId");
        assert!(
            is_association_id(&association_id),
            "case {:?}: expected an AssociationId of the modelled form, got {:?}",
            row.case,
            association_id
        );
        assert_eq!(
            text(&created, "Name"),
            row.name,
            "case {:?}: expected the association to report the document name, got {created:?}",
            row.case
        );
        for (member, wanted) in &row.echoed {
            assert_eq!(
                &created[*member], wanted,
                "case {:?}: expected {member} to be {wanted:?}, got {:?}",
                row.case, created[*member]
            );
        }
        for member in &row.unset {
            assert!(
                created[*member].is_null(),
                "case {:?}: expected {member} to be unset, got {:?}",
                row.case,
                created[*member]
            );
        }
        // "The date when the association was made", and the date it was last updated,
        // which for an association nothing has updated is the same instant.
        assert!(
            created["Date"].is_number(),
            "case {:?}: expected a creation date, got {created:?}",
            row.case
        );
        assert_eq!(
            created["Date"], created["LastUpdateAssociationDate"],
            "case {:?}: an association nothing has updated was last updated when it was \
             created, got {created:?}",
            row.case
        );
        // An association nothing has run reports its own progress and no execution count.
        assert_eq!(
            created["Overview"],
            json!({"Status": "Pending", "DetailedStatus": "Creating"}),
            "case {:?}: expected the overview of an association being created, got {:?}",
            row.case,
            created["Overview"]
        );
        assert_eq!(
            stored_associations(&engine),
            1,
            "case {:?}: the create persists exactly one association",
            row.case
        );

        // The round trip: the reader reports the association the create persisted, by its
        // id, and by every selector that names its one version. The overview is the one
        // member the clock moves, so it is asserted for what holds at every instant.
        for selector in [None, Some("$LATEST"), Some("1")] {
            let mut request = json!({"AssociationId": association_id});
            if let Some(selector) = selector {
                request["AssociationVersion"] = json!(selector);
            }
            let read = described(&router, row.case, &request).await;
            assert_eq!(
                without_overview(&read),
                without_overview(&created),
                "case {:?}: a read at version {selector:?} reports the association the \
                 create persisted, got {read:?}",
                row.case
            );
            assert_progress_only(&read, row.case);
        }
    }
}

/// An association is reached by the id its create minted, through the Region it was
/// created in, and an id nothing was created under names none.
#[tokio::test]
async fn a_read_resolves_an_association_by_the_id_its_create_minted() {
    let (router, _engine) = service();
    let first = created(
        &router,
        "the first association",
        &json!({"Name": AGENT, "Targets": [{"Key": "InstanceIds", "Values": [NODE]}]}),
    )
    .await;
    let second = created(
        &router,
        "the second association",
        &json!({"Name": AGENT, "Targets": [{"Key": "InstanceIds", "Values": [OTHER_NODE]}]}),
    )
    .await;
    let first_id = text(&first, "AssociationId");
    let second_id = text(&second, "AssociationId");
    assert_ne!(
        first_id, second_id,
        "each create mints its own association id, got {first_id} twice"
    );

    // One row per way an id can fail to name an association reachable here: never minted,
    // the other association's Region, and the id of an association of another Region.
    let rows = vec![
        (
            "an id nothing was created under",
            REGION,
            "11111111-2222-3333-4444-555555555555".to_string(),
        ),
        ("an id of another Region", OTHER_REGION, first_id.clone()),
        ("another Region's view of an id", OTHER_REGION, second_id),
    ];
    for (case, region, association_id) in rows {
        let (status, parsed) =
            describe_in_region(&router, region, &json!({"AssociationId": association_id})).await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, "AssociationDoesNotExist"),
            "case {case:?}: expected the association to be reported as nonexistent, got \
             {status} with {parsed:?}"
        );
        assert_eq!(
            error_message(&parsed),
            associations::ASSOCIATION_DOES_NOT_EXIST,
            "case {case:?}: expected the modelled refusal sentence, got {parsed:?}"
        );
    }

    // The association created in this Region is still there, and still its own.
    let read = described(
        &router,
        "the association of this Region",
        &json!({"AssociationId": first_id}),
    )
    .await;
    assert_eq!(
        without_overview(&read),
        without_overview(&first),
        "the read reports the association created in this Region, got {read:?}"
    );
}

/// An association created for a node the deprecated way is also reached by the document
/// and the node it pairs, and a version it does not hold is refused.
#[tokio::test]
async fn an_association_is_reached_by_the_document_and_node_it_pairs() {
    let (router, _engine) = service();
    let created = created(
        &router,
        "the node's association",
        &json!({"Name": AGENT, "InstanceId": NODE}),
    )
    .await;

    // One row per way of naming the pair: both members (which is what identifies the
    // association), each member alone — which identifies no pair, so it is refused as a
    // malformed request — and a pair the store holds nothing for.
    let rows: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "the document and the node",
            json!({"Name": AGENT, "InstanceId": NODE}),
            None,
        ),
        (
            "the document alone",
            json!({"Name": AGENT}),
            Some("ValidationException"),
        ),
        (
            "the node alone",
            json!({"InstanceId": NODE}),
            Some("ValidationException"),
        ),
        (
            "another document on that node",
            json!({"Name": SHELL, "InstanceId": NODE}),
            Some("AssociationDoesNotExist"),
        ),
        (
            "that document on another node",
            json!({"Name": AGENT, "InstanceId": OTHER_NODE}),
            Some("AssociationDoesNotExist"),
        ),
    ];
    for (case, request, refusal) in rows {
        let (status, parsed) = describe_in_region(&router, REGION, &request).await;
        match refusal {
            None => assert_eq!(
                (status, without_overview(&parsed["AssociationDescription"])),
                (200, without_overview(&created)),
                "case {case:?}: expected the node's association, got {status} with {parsed:?}"
            ),
            Some(code) => assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, code),
                "case {case:?}: expected {code}, got {status} with {parsed:?}"
            ),
        }
    }

    // A version selector the association holds nothing for, and a request naming no
    // association at all.
    let association_id = text(&created, "AssociationId");
    for version in ["2", "17"] {
        let (status, parsed) = describe_in_region(
            &router,
            REGION,
            &json!({"AssociationId": association_id, "AssociationVersion": version}),
        )
        .await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, "InvalidAssociationVersion"),
            "case {version:?}: expected a version the association doesn't hold to be \
             refused, got {status} with {parsed:?}"
        );
    }
    let (status, parsed) = describe_in_region(&router, REGION, &json!({})).await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "ValidationException"),
        "a read naming no association is refused, got {status} with {parsed:?}"
    );
}

/// One create naming a document, and whether the document and version it names resolve.
struct DocumentRow {
    case: &'static str,
    /// The `Name` the create carries: a document name or an ARN spelling of one.
    name: &'static str,
    /// The `DocumentVersion` the create names, if any.
    version: Option<&'static str>,
    /// The parameter values that document declares, so the parameter rule does not decide
    /// this suite's rows.
    parameters: Value,
    /// The error code the create is refused with, or `None` when it is served.
    refusal: Option<&'static str>,
}

/// The document an association applies is resolved among the documents the Region holds
/// and the ones Systems Manager publishes, at a version that document carries.
#[tokio::test]
async fn a_document_the_region_holds_none_of_or_a_version_it_lacks_is_refused() {
    let (router, engine) = service();
    // A document of the account's own, so the resolution spans created and published
    // documents alike.
    let own = "Echo-Message";
    let (status, parsed) = call(
        &router,
        "CreateDocument",
        &json!({
            "Name": own,
            "DocumentType": "Command",
            "Content": r#"{"schemaVersion":"2.2","description":"echo","mainSteps":[{"action":"aws:runShellScript","name":"runShell","inputs":{"runCommand":["echo hi"]}}]}"#,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the account's own document must be created, got {status} with {parsed:?}"
    );

    // One row per (document, version) the create may name: the account's own document, a
    // published command document, a published automation document, each at every selector
    // a one-version document carries, plus the names and versions that resolve to nothing.
    // The parameters each document declares are what the create supplies; whether a
    // parameter set satisfies a document is its own suite below.
    let rows: Vec<DocumentRow> = vec![
        DocumentRow {
            case: "the account's own document",
            name: own,
            version: None,
            parameters: json!({}),
            refusal: None,
        },
        DocumentRow {
            case: "a published command document",
            name: AGENT,
            version: Some("$DEFAULT"),
            parameters: json!({}),
            refusal: None,
        },
        DocumentRow {
            case: "a published automation document",
            name: AUTOMATION,
            version: Some("$LATEST"),
            parameters: json!({"BucketName": ["logs-bucket"]}),
            refusal: None,
        },
        DocumentRow {
            case: "the first version by number",
            name: AGENT,
            version: Some("1"),
            parameters: json!({}),
            refusal: None,
        },
        DocumentRow {
            case: "a name nothing is held under",
            name: "Absent-Document",
            version: None,
            parameters: json!({}),
            refusal: Some("InvalidDocument"),
        },
        DocumentRow {
            case: "an ARN of another account",
            name: "arn:aws:ssm:us-west-2:999988887777:document/AWS-UpdateSSMAgent",
            version: None,
            parameters: json!({}),
            refusal: Some("InvalidDocument"),
        },
        DocumentRow {
            case: "a version the document doesn't carry",
            name: AGENT,
            version: Some("2"),
            parameters: json!({}),
            refusal: Some("InvalidDocumentVersion"),
        },
        DocumentRow {
            case: "a far later version",
            name: own,
            version: Some("9"),
            parameters: json!({}),
            refusal: Some("InvalidDocumentVersion"),
        },
    ];
    let mut created_count = 0;
    for DocumentRow {
        case,
        name,
        version,
        parameters,
        refusal,
    } in rows
    {
        let mut request = json!({"Name": name});
        if let Some(version) = version {
            request["DocumentVersion"] = json!(version);
        }
        if parameters != json!({}) {
            request["Parameters"] = parameters;
        }
        let (status, parsed) = create(&router, &request).await;
        match refusal {
            None => {
                created_count += 1;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the create to be served, got {status} with \
                     {parsed:?}"
                );
            }
            Some(code) => assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, code),
                "case {case:?}: expected {code}, got {status} with {parsed:?}"
            ),
        }
        assert_eq!(
            stored_associations(&engine),
            created_count,
            "case {case:?}: a refused create persists no association"
        );
    }
}

/// A create supplies values for the parameters the document version declares, and only for
/// those.
#[tokio::test]
async fn parameters_the_document_doesnt_declare_or_requires_are_refused() {
    // One row per parameter set against the document that declares `commands` with no
    // default: the required value supplied, left out, misspelled, and an extra name.
    let rows: Vec<(&str, &str, Value, bool)> = vec![
        (
            "the required parameter supplied",
            SHELL,
            json!({"commands": ["echo hi"]}),
            true,
        ),
        (
            "the required parameter left out",
            SHELL,
            json!({}),
            false,
        ),
        (
            "a parameter the document doesn't declare",
            SHELL,
            json!({"commands": ["echo hi"], "shell": ["bash"]}),
            false,
        ),
        (
            "a document whose every parameter has a default, with none supplied",
            AGENT,
            json!({}),
            true,
        ),
        (
            "a declared parameter of that document supplied",
            AGENT,
            json!({"version": ["3.1.1188.0"]}),
            true,
        ),
        (
            "an undeclared parameter of that document",
            AGENT,
            json!({"nonesuch": ["1"]}),
            false,
        ),
    ];

    for (case, name, parameters, served) in rows {
        let (router, engine) = service();
        let mut request = json!({"Name": name});
        if parameters != json!({}) {
            request["Parameters"] = parameters;
        }
        let (status, parsed) = create(&router, &request).await;
        if served {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the create to be served, got {status} with {parsed:?}"
            );
        } else {
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "InvalidParameters"),
                "case {case:?}: expected InvalidParameters, got {status} with {parsed:?}"
            );
            assert_eq!(
                stored_associations(&engine),
                0,
                "case {case:?}: a refused create persists no association"
            );
        }
    }
}

/// A schedule has to be one of the two forms a State Manager association is applied on.
#[tokio::test]
async fn a_schedule_has_to_be_a_cron_or_rate_expression() {
    // One row per form: cron expressions over the six fields such an expression carries,
    // rate expressions at each unit and at the shortest interval allowed, and expressions
    // in neither form.
    let rows: Vec<(&str, &str, bool)> = vec![
        ("a daily cron expression", "cron(0 2 ? * * *)", true),
        ("a cron expression on a weekday", "cron(15 10 ? * MON-FRI *)", true),
        ("a cron expression on the second Thursday", "cron(0 0 ? * THU#2 *)", true),
        ("a cron expression with a step", "cron(0/30 * ? * * *)", true),
        ("the shortest rate allowed", "rate(30 minutes)", true),
        ("a rate in hours", "rate(1 hour)", true),
        ("a rate in days", "rate(7 days)", true),
        ("a cron expression of five fields", "cron(0 2 ? * SUN)", false),
        ("a cron expression of seven fields", "cron(0 0 2 ? * SUN *)", false),
        ("an empty cron expression", "cron()", false),
        ("a rate below the shortest allowed", "rate(5 minutes)", false),
        ("a rate of no interval at all", "rate(0 minutes)", false),
        ("a rate whose unit is misspelled", "rate(2 hour)", false),
        ("a rate whose unit is unknown", "rate(2 fortnights)", false),
        (
            "a rate whose interval does not fit",
            "rate(18446744073709551615 days)",
            false,
        ),
        (
            "a rate value no number can hold",
            "rate(99999999999999999999999 minutes)",
            false,
        ),
        ("a schedule in neither form", "every 30 minutes", false),
        ("a cron expression that is not wrapped", "0 2 ? * * *", false),
    ];

    for (case, expression, served) in rows {
        let (router, engine) = service();
        let (status, parsed) = create(
            &router,
            &json!({"Name": AGENT, "ScheduleExpression": expression}),
        )
        .await;
        if served {
            assert_eq!(
                status, 200,
                "case {case:?} ({expression}): expected the schedule to be served, got \
                 {status} with {parsed:?}"
            );
            assert_eq!(
                text(&parsed["AssociationDescription"], "ScheduleExpression"),
                expression,
                "case {case:?}: the association is applied on the schedule it was created \
                 with, got {parsed:?}"
            );
        } else {
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "InvalidSchedule"),
                "case {case:?} ({expression}): expected InvalidSchedule, got {status} with \
                 {parsed:?}"
            );
            assert_eq!(
                error_message(&parsed),
                associations::INVALID_SCHEDULE,
                "case {case:?}: expected the modelled refusal sentence, got {parsed:?}"
            );
            assert_eq!(
                stored_associations(&engine),
                0,
                "case {case:?}: a refused create persists no association"
            );
        }
    }
}

/// A node named the deprecated way carries the association of one document once; the
/// output location has to name a bucket, and targets and target maps cannot both be named.
#[tokio::test]
async fn a_create_the_store_cannot_hold_is_refused() {
    let (router, engine) = service();
    created(
        &router,
        "the node's first association",
        &json!({"Name": AGENT, "InstanceId": NODE}),
    )
    .await;

    // One row per create against that state: the same pair again, the same document on
    // another node, another document on the same node, the same document targeted by
    // criteria rather than paired with the node, and the two malformed creates.
    let rows: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "the same document on the same node",
            json!({"Name": AGENT, "InstanceId": NODE}),
            Some("AssociationAlreadyExists"),
        ),
        (
            "the same document on another node",
            json!({"Name": AGENT, "InstanceId": OTHER_NODE}),
            None,
        ),
        (
            "another document on the same node",
            json!({"Name": SHELL, "InstanceId": NODE, "Parameters": {"commands": ["echo hi"]}}),
            None,
        ),
        (
            "the same document targeted by criteria",
            json!({"Name": AGENT, "Targets": [{"Key": "InstanceIds", "Values": [NODE]}]}),
            None,
        ),
        (
            "criteria naming the same node a second time",
            json!({"Name": AGENT, "Targets": [{"Key": "InstanceIds", "Values": [NODE]}]}),
            None,
        ),
        (
            "an output location naming no bucket",
            json!({"Name": AGENT, "OutputLocation": {"S3Location": {"OutputS3KeyPrefix": "out"}}}),
            Some("InvalidOutputLocation"),
        ),
        (
            "an output location whose S3 location names only a Region",
            json!({"Name": AGENT, "OutputLocation": {"S3Location": {"OutputS3Region": REGION}}}),
            Some("InvalidOutputLocation"),
        ),
        (
            "targets and target maps together",
            json!({
                "Name": AGENT,
                "Targets": [{"Key": "InstanceIds", "Values": [NODE]}],
                "TargetMaps": [{"BucketName": ["logs"]}],
            }),
            Some("InvalidTargetMaps"),
        ),
    ];

    let mut held = stored_associations(&engine);
    for (case, request, refusal) in rows {
        let (status, parsed) = create(&router, &request).await;
        match refusal {
            None => {
                held += 1;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the create to be served, got {status} with \
                     {parsed:?}"
                );
            }
            Some(code) => assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, code),
                "case {case:?}: expected {code}, got {status} with {parsed:?}"
            ),
        }
        assert_eq!(
            stored_associations(&engine),
            held,
            "case {case:?}: only a served create persists an association"
        );
    }
}

/// An association reports the move off `Creating` the clock already owes, whether or not
/// anybody read it in between.
#[tokio::test]
async fn an_association_reports_the_creation_progress_the_clock_owes() {
    // One row per age of the same association: just created, halfway through the span,
    // and past it.
    let rows = vec![
        ("just created", Duration::ZERO, Some("Creating")),
        (
            "halfway through the creation span",
            associations::ASSOCIATION_CREATION_DELAY / 2,
            Some("Creating"),
        ),
        (
            "just past the creation span",
            associations::ASSOCIATION_CREATION_DELAY + Duration::from_millis(500),
            None,
        ),
        (
            "an hour old",
            Duration::from_secs(60 * 60),
            None,
        ),
    ];

    for (case, age, detailed_status) in rows {
        let (router, engine) = service();
        let created = created(&router, case, &json!({"Name": AGENT})).await;
        let association_id = text(&created, "AssociationId");
        // Age the stored association rather than the clock: what the store holds is what a
        // read reports, so an association created that long ago is this association.
        {
            let state = engine.state::<ComAmazonawsSsmAmazonssmState>();
            let mut store = state.lock();
            for held in store.associations.values_mut() {
                held.created_at -= age;
                for version in &mut held.versions {
                    version.created_at -= age;
                }
            }
        }
        let read = described(&router, case, &json!({"AssociationId": association_id})).await;
        assert_eq!(
            read["Overview"]["Status"],
            json!("Pending"),
            "case {case:?}: an association nothing has run is Pending, got {read:?}"
        );
        match detailed_status {
            Some(wanted) => assert_eq!(
                read["Overview"]["DetailedStatus"],
                json!(wanted),
                "case {case:?}: expected the detailed status {wanted}, got {read:?}"
            ),
            None => assert!(
                read["Overview"]["DetailedStatus"].is_null(),
                "case {case:?}: expected no detailed status once the create has settled, \
                 got {read:?}"
            ),
        }
        // Nothing has run the association, at any age, so no execution is reported.
        assert!(
            read["Overview"]["AssociationStatusAggregatedCount"].is_null()
                && read["LastExecutionDate"].is_null(),
            "case {case:?}: an association nothing has run reports no execution, got {read:?}"
        );
    }
}

/// One create's tag set, and the tags the association then carries.
struct TagRow {
    case: &'static str,
    /// The `Tags` the create carries.
    tags: Value,
    /// The tag set the association reports, in the key order the store holds it in.
    expected: Vec<(&'static str, &'static str)>,
}

/// The tags a create wrote are the association's own tag set: what the tag operations read,
/// overwrite and remove through the `Association` resource type.
#[tokio::test]
async fn the_tags_a_create_wrote_are_the_tags_the_tag_operations_read() {
    // One row per tag set a create may carry.
    let rows: Vec<TagRow> = vec![
        TagRow {
            case: "no tags at all",
            tags: json!([]),
            expected: vec![],
        },
        TagRow {
            case: "one tag",
            tags: json!([{"Key": "Env", "Value": "prod"}]),
            expected: vec![("Env", "prod")],
        },
        TagRow {
            case: "several tags, reported in key order",
            tags: json!([
                {"Key": "Owner", "Value": "platform"},
                {"Key": "Env", "Value": "dev"},
            ]),
            expected: vec![("Env", "dev"), ("Owner", "platform")],
        },
    ];

    for TagRow {
        case,
        tags,
        expected: wanted,
    } in rows
    {
        let (router, _engine) = service();
        let mut request = json!({"Name": AGENT});
        if tags != json!([]) {
            request["Tags"] = tags;
        }
        let created = created(&router, case, &request).await;
        let association_id = text(&created, "AssociationId");

        let expected: Value = wanted
            .iter()
            .map(|(key, value)| json!({"Key": key, "Value": value}))
            .collect();
        let (status, parsed) = call(
            &router,
            "ListTagsForResource",
            &json!({"ResourceType": "Association", "ResourceId": association_id}),
        )
        .await;
        assert_eq!(
            (status, &parsed["TagList"]),
            (200, &expected),
            "case {case:?}: expected the association's tag set, got {status} with {parsed:?}"
        );

        // A tag written through the tag plane lands on the same association, and a key it
        // already carries takes the new value rather than appearing twice.
        let (status, parsed) = call(
            &router,
            "AddTagsToResource",
            &json!({
                "ResourceType": "Association",
                "ResourceId": association_id,
                "Tags": [{"Key": "Env", "Value": "staging"}],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: tagging the association must be served, got {status} with \
             {parsed:?}"
        );
        let (status, parsed) = call(
            &router,
            "ListTagsForResource",
            &json!({"ResourceType": "Association", "ResourceId": association_id}),
        )
        .await;
        let reported = parsed["TagList"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|tag| {
                Some((
                    tag["Key"].as_str()?.to_string(),
                    tag["Value"].as_str()?.to_string(),
                ))
            })
            .collect::<Vec<(String, String)>>();
        assert_eq!(
            (status, reported.iter().filter(|(key, _)| key == "Env").count()),
            (200, 1),
            "case {case:?}: a key the association already carries takes the new value, got \
             {parsed:?}"
        );
        assert!(
            reported.contains(&("Env".to_string(), "staging".to_string())),
            "case {case:?}: expected the written tag on the association, got {parsed:?}"
        );
    }
}

/// How many associations one Region holds is bounded by the documented limit, and the store
/// as a whole by its own total guard; a create past either is refused and persists nothing.
#[tokio::test]
async fn the_associations_a_region_and_the_store_hold_are_bounded() {
    let (router, engine) = service();

    // Fill this Region to the documented limit, through the front door, so every record
    // counted is one a create persisted.
    for sequence in 0..associations::MAX_ASSOCIATIONS_PER_REGION {
        let (status, parsed) = create(&router, &json!({"Name": AGENT})).await;
        assert_eq!(
            status, 200,
            "create {sequence}: a create inside the limit is served, got {status} with \
             {parsed:?}"
        );
    }
    let held = stored_associations(&engine);
    assert_eq!(
        held,
        associations::MAX_ASSOCIATIONS_PER_REGION,
        "the Region holds the limit's worth of associations"
    );

    // One past the documented limit: refused with the modelled error, which carries no
    // message member, and nothing is persisted.
    let (status, parsed) = create(&router, &json!({"Name": AGENT})).await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "AssociationLimitExceeded"),
        "a create past the Region's limit is refused, got {status} with {parsed:?}"
    );
    assert_eq!(
        stored_associations(&engine),
        held,
        "a create past the Region's limit persists no association"
    );

    // The limit is measured per Region, so another Region's endpoint still serves creates
    // until the store's own total guard is reached.
    let (status, parsed) =
        call_in_region(&router, OTHER_REGION, "CreateAssociation", &json!({"Name": AGENT})).await;
    assert_eq!(
        status, 200,
        "the limit is per Region, so another Region still serves a create, got {status} \
         with {parsed:?}"
    );

    // Fill the store to its total guard, then one more: refused as a request this emulator
    // cannot hold, and still nothing is persisted.
    while stored_associations(&engine) < associations::MAX_ASSOCIATIONS_TOTAL {
        let (status, parsed) =
            call_in_region(&router, OTHER_REGION, "CreateAssociation", &json!({"Name": AGENT}))
                .await;
        assert_eq!(
            status, 200,
            "a create inside the store's total guard is served, got {status} with {parsed:?}"
        );
    }
    let held = stored_associations(&engine);
    let (status, parsed) =
        call_in_region(&router, THIRD_REGION, "CreateAssociation", &json!({"Name": AGENT})).await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "ValidationException"),
        "a create past the store's total guard is refused, got {status} with {parsed:?}"
    );
    assert_eq!(
        stored_associations(&engine),
        held,
        "a create past the store's total guard persists no association"
    );
}
