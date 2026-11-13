// SPDX-License-Identifier: CC0-1.0

//! The DataSync transfer-task lifecycle, driven end to end over HTTP.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: FmrsService.<Operation>` and an `awsJson1_1` body, dispatched by the
//! multi-service router. Nothing reaches into the state struct, so a response that is
//! not computed from persisted state cannot pass.

use std::sync::Arc;
use std::time::Duration;

use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite signs for. It is deliberately NOT the
/// emulator's default, so an ARN that carries it proves the region came from the
/// request's SigV4 credential scope.
const SIGNED_REGION: &str = "us-west-2";

/// The canonical account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// Long enough for one clock-owed transition (`state::TRANSITION_SPAN` is 300 ms) to
/// fall due, with margin for a loaded test machine.
const ONE_SPAN: Duration = Duration::from_millis(500);

/// Long enough for all four phases of a task execution to fall due.
const ALL_PHASES: Duration = Duration::from_millis(1_800);

/// A router with just this service mounted on a fresh engine.
fn service() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_datasync_fmrsservice::EmulatedService::new(engine),
    );
    router
}

/// One `awsJson1_1` call, returning the HTTP status and the parsed response body.
async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("FmrsService.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDDATASYNCTEST/20260101/{SIGNED_REGION}\
                 /datasync/aws4_request, SignedHeaders=host;x-amz-target, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_slice(&payload).unwrap_or_else(|_| {
            serde_json::json!({ "__unparsed": String::from_utf8_lossy(&payload) })
        })
    };
    (status, parsed)
}

/// A call that must succeed, returning its body.
async fn ok(
    router: &MultiServiceRouter,
    operation: &str,
    body: serde_json::Value,
    case: &str,
) -> serde_json::Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer 200, got {status} with {parsed}"
    );
    parsed
}

/// A call that must be refused with HTTP 400 `InvalidRequestException`.
async fn invalid_request(
    router: &MultiServiceRouter,
    operation: &str,
    body: serde_json::Value,
    case: &str,
) {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected {operation} to answer 400, got {status} with {parsed}"
    );
    let wire_type = parsed["__type"].as_str().unwrap_or_default();
    assert!(
        wire_type.contains("InvalidRequestException"),
        "case {case:?}: expected an InvalidRequestException from {operation}, got {parsed}"
    );
}

/// A required string member of a response body.
fn string(body: &serde_json::Value, member: &str, case: &str) -> String {
    body[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string {member}, got {body}"))
        .to_string()
}

/// Assert an id suffix is `len` characters drawn from `[0-9a-z]` (the modelled location
/// id charset) or `[0-9a-f]` when `hex_only`.
fn assert_id(id: &str, len: usize, hex_only: bool, case: &str) {
    let charset_ok = id.bytes().all(|b| {
        b.is_ascii_digit() || if hex_only { (b'a'..=b'f').contains(&b) } else { b.is_ascii_lowercase() }
    });
    assert!(
        id.len() == len && charset_ok,
        "case {case:?}: expected a {len}-character {} id, got {id:?}",
        if hex_only { "hexadecimal" } else { "lowercase alphanumeric" }
    );
}

/// The modelled `LocationArn` shape: `…:location/loc-[0-9a-z]{17}`.
fn assert_location_arn(arn: &str, case: &str) {
    let prefix = format!("arn:aws:datasync:{SIGNED_REGION}:{ACCOUNT}:location/loc-");
    assert!(
        arn.starts_with(&prefix),
        "case {case:?}: expected a location ARN starting {prefix:?}, got {arn:?}"
    );
    assert_id(&arn[prefix.len()..], 17, false, case);
}

/// The modelled `TaskArn` shape: `…:task/task-[0-9a-f]{17}`.
fn assert_task_arn(arn: &str, case: &str) {
    let prefix = format!("arn:aws:datasync:{SIGNED_REGION}:{ACCOUNT}:task/task-");
    assert!(
        arn.starts_with(&prefix),
        "case {case:?}: expected a task ARN starting {prefix:?}, got {arn:?}"
    );
    assert_id(&arn[prefix.len()..], 17, true, case);
}

/// The modelled `TaskExecutionArn` shape: the task's own ARN plus
/// `/execution/exec-[0-9a-f]{17}`.
fn assert_execution_arn(arn: &str, task_arn: &str, case: &str) {
    let prefix = format!("{task_arn}/execution/exec-");
    assert!(
        arn.starts_with(&prefix),
        "case {case:?}: expected an execution ARN starting {prefix:?}, got {arn:?}"
    );
    assert_id(&arn[prefix.len()..], 17, true, case);
}

/// A plain-bucket S3 location, created and returned by ARN.
async fn create_bucket_location(
    router: &MultiServiceRouter,
    bucket: &str,
    subdirectory: &str,
    case: &str,
) -> String {
    let created = ok(
        router,
        "CreateLocationS3",
        serde_json::json!({
            "S3BucketArn": format!("arn:aws:s3:::{bucket}"),
            "S3Config": { "BucketAccessRoleArn": "arn:aws:iam::000000000000:role/datasync" },
            "Subdirectory": subdirectory,
        }),
        case,
    )
    .await;
    let arn = string(&created, "LocationArn", case);
    assert_location_arn(&arn, case);
    arn
}

/// A task between two fresh locations, created and returned by ARN.
async fn create_task(router: &MultiServiceRouter, name: &str) -> String {
    let source = create_bucket_location(router, "lifecycle-source", "/in", name).await;
    let destination = create_bucket_location(router, "lifecycle-destination", "/out", name).await;
    let created = ok(
        router,
        "CreateTask",
        serde_json::json!({
            "SourceLocationArn": source,
            "DestinationLocationArn": destination,
            "Name": name,
        }),
        name,
    )
    .await;
    let arn = string(&created, "TaskArn", name);
    assert_task_arn(&arn, name);
    arn
}

/// The whole slice the bootstrap issue specifies, in order, asserting bodies at each
/// step: create two locations, read them back, create a task, watch it leave
/// `CREATING`, start an execution, watch the execution walk its phases to `SUCCESS`,
/// confirm the task points at it, then delete the task and confirm it is gone.
#[tokio::test]
async fn transfer_task_lifecycle_round_trips_over_http() {
    let case = "lifecycle";
    let router = service();

    let source = create_bucket_location(&router, "demo-source", "/in", case).await;
    let destination = create_bucket_location(&router, "demo-destination", "/out", case).await;
    assert_ne!(
        source, destination,
        "case {case:?}: two creates must mint two distinct location ARNs, got {source:?} twice"
    );

    // Round-trip the create through the describe: the stored record, not an echo.
    let described = ok(
        &router,
        "DescribeLocationS3",
        serde_json::json!({ "LocationArn": source }),
        case,
    )
    .await;
    assert_eq!(
        string(&described, "LocationArn", case),
        source,
        "case {case:?}: DescribeLocationS3 must report the ARN it was asked about, got {described}"
    );
    assert_eq!(
        string(&described, "LocationUri", case),
        "s3://demo-source/in/",
        "case {case:?}: expected the stored LocationUri, got {described}"
    );
    assert_eq!(
        described["S3Config"]["BucketAccessRoleArn"]
            .as_str()
            .unwrap_or_default(),
        "arn:aws:iam::000000000000:role/datasync",
        "case {case:?}: expected the stored S3Config, got {described}"
    );
    assert!(
        described["CreationTime"].is_number(),
        "case {case:?}: expected a CreationTime timestamp, got {described}"
    );

    let created = ok(
        &router,
        "CreateTask",
        serde_json::json!({
            "SourceLocationArn": source,
            "DestinationLocationArn": destination,
            "Name": "demo-task",
            "Includes": [{ "FilterType": "SIMPLE_PATTERN", "Value": "/keep" }],
            "Options": { "LogLevel": "TRANSFER", "BytesPerSecond": 1_048_576 },
        }),
        case,
    )
    .await;
    let task_arn = string(&created, "TaskArn", case);
    assert_task_arn(&task_arn, case);

    // A caller acting straight after CreateTask still sees the transient status, and
    // the state error that goes with it.
    let describe = serde_json::json!({ "TaskArn": task_arn });
    let creating = ok(&router, "DescribeTask", describe.clone(), case).await;
    assert_eq!(
        string(&creating, "Status", case),
        "CREATING",
        "case {case:?}: a task must start in CREATING, got {creating}"
    );
    invalid_request(
        &router,
        "StartTaskExecution",
        serde_json::json!({ "TaskArn": task_arn }),
        "start while CREATING",
    )
    .await;

    std::thread::sleep(ONE_SPAN);
    let available = ok(&router, "DescribeTask", describe.clone(), case).await;
    assert_eq!(
        string(&available, "Status", case),
        "AVAILABLE",
        "case {case:?}: a task must reach AVAILABLE on its own, got {available}"
    );
    // DescribeTask reports exactly what CreateTask was given.
    assert_eq!(
        string(&available, "Name", case),
        "demo-task",
        "case {case:?}: expected the Name given to CreateTask, got {available}"
    );
    assert_eq!(
        string(&available, "SourceLocationArn", case),
        source,
        "case {case:?}: expected the source location given to CreateTask, got {available}"
    );
    assert_eq!(
        string(&available, "DestinationLocationArn", case),
        destination,
        "case {case:?}: expected the destination location given to CreateTask, got {available}"
    );
    assert_eq!(
        available["Includes"][0]["Value"].as_str().unwrap_or_default(),
        "/keep",
        "case {case:?}: expected the Includes given to CreateTask, got {available}"
    );
    assert_eq!(
        available["Options"]["LogLevel"].as_str().unwrap_or_default(),
        "TRANSFER",
        "case {case:?}: expected the overridden LogLevel, got {available}"
    );
    assert_eq!(
        string(&available, "TaskMode", case),
        "BASIC",
        "case {case:?}: expected the modelled default TaskMode, got {available}"
    );
    assert!(
        available["CreationTime"].is_number(),
        "case {case:?}: expected a CreationTime timestamp, got {available}"
    );
    assert!(
        available["CurrentTaskExecutionArn"].is_null(),
        "case {case:?}: a task that never ran has no current execution, got {available}"
    );

    let started = ok(
        &router,
        "StartTaskExecution",
        serde_json::json!({
            "TaskArn": task_arn,
            "OverrideOptions": { "LogLevel": "BASIC" },
        }),
        case,
    )
    .await;
    let execution_arn = string(&started, "TaskExecutionArn", case);
    assert_execution_arn(&execution_arn, &task_arn, case);

    let describe_execution = serde_json::json!({ "TaskExecutionArn": execution_arn });
    let launching = ok(&router, "DescribeTaskExecution", describe_execution.clone(), case).await;
    assert_eq!(
        string(&launching, "Status", case),
        "LAUNCHING",
        "case {case:?}: a just-started execution is still LAUNCHING, got {launching}"
    );
    assert!(
        launching["StartTime"].is_number(),
        "case {case:?}: expected a StartTime timestamp, got {launching}"
    );
    assert!(
        launching["EndTime"].is_null(),
        "case {case:?}: a running execution has no EndTime, got {launching}"
    );
    // The options actually in effect: the task's, with this run's override applied.
    assert_eq!(
        launching["Options"]["LogLevel"].as_str().unwrap_or_default(),
        "BASIC",
        "case {case:?}: expected OverrideOptions to win for LogLevel, got {launching}"
    );
    assert_eq!(
        launching["Options"]["BytesPerSecond"].as_i64().unwrap_or_default(),
        1_048_576,
        "case {case:?}: expected the task's BytesPerSecond to survive the override, got {launching}"
    );
    for counter in [
        "EstimatedFilesToTransfer",
        "EstimatedBytesToTransfer",
        "FilesTransferred",
        "BytesWritten",
        "BytesTransferred",
        "BytesCompressed",
        "FilesDeleted",
        "FilesSkipped",
        "FilesVerified",
        "EstimatedFilesToDelete",
        "FilesPrepared",
    ] {
        assert!(
            launching[counter].is_number(),
            "case {case:?}: expected the required counter {counter}, got {launching}"
        );
    }

    let running = ok(&router, "DescribeTask", describe.clone(), case).await;
    assert_eq!(
        string(&running, "Status", case),
        "RUNNING",
        "case {case:?}: StartTaskExecution must move the task to RUNNING, got {running}"
    );
    assert_eq!(
        string(&running, "CurrentTaskExecutionArn", case),
        execution_arn,
        "case {case:?}: expected the started execution as CurrentTaskExecutionArn, got {running}"
    );

    std::thread::sleep(ALL_PHASES);
    let finished = ok(&router, "DescribeTaskExecution", describe_execution.clone(), case).await;
    assert_eq!(
        string(&finished, "Status", case),
        "SUCCESS",
        "case {case:?}: an execution must reach SUCCESS on its own, got {finished}"
    );
    assert!(
        finished["EndTime"].is_number(),
        "case {case:?}: a finished execution reports an EndTime, got {finished}"
    );
    assert!(
        finished["LaunchTime"].is_number(),
        "case {case:?}: a launched execution reports a LaunchTime, got {finished}"
    );
    for phase in ["PrepareStatus", "TransferStatus", "VerifyStatus"] {
        assert_eq!(
            finished["Result"][phase].as_str().unwrap_or_default(),
            "SUCCESS",
            "case {case:?}: expected {phase} SUCCESS once the execution ended, got {finished}"
        );
    }
    let total = finished["Result"]["TotalDuration"].as_i64().unwrap_or(-1);
    assert!(
        total > 0,
        "case {case:?}: expected a positive TotalDuration, got {total}"
    );

    let back_to_available = ok(&router, "DescribeTask", describe.clone(), case).await;
    assert_eq!(
        string(&back_to_available, "Status", case),
        "AVAILABLE",
        "case {case:?}: a task is AVAILABLE again once its execution ends, got {back_to_available}"
    );

    ok(
        &router,
        "DeleteTask",
        serde_json::json!({ "TaskArn": task_arn }),
        case,
    )
    .await;
    invalid_request(&router, "DescribeTask", describe.clone(), "describe after delete").await;
    invalid_request(
        &router,
        "DeleteTask",
        serde_json::json!({ "TaskArn": task_arn }),
        "delete twice",
    )
    .await;
    invalid_request(
        &router,
        "DescribeTaskExecution",
        describe_execution,
        "describe execution of a deleted task",
    )
    .await;
    let listed = ok(&router, "ListTasks", serde_json::json!({}), case).await;
    assert_eq!(
        listed["Tasks"].as_array().map(Vec::len),
        Some(0),
        "case {case:?}: a deleted task must be absent from ListTasks, got {listed}"
    );
}

/// `CreateLocationS3` stores the location it mints, and `DescribeLocationS3` reports it.
///
/// The rows span what the rule generalises over: the three ARN forms the
/// `S3BucketArn` member models (bucket, access point, Outposts access point), the
/// `Subdirectory` spellings that denote one prefix, and a declared storage class versus
/// the per-ARN-form default the operation documents.
#[tokio::test]
async fn created_s3_locations_are_reported_from_state() {
    struct Row {
        case: &'static str,
        bucket_arn: &'static str,
        subdirectory: Option<&'static str>,
        storage_class: Option<&'static str>,
        expected_uri: &'static str,
        expected_storage_class: &'static str,
    }

    let rows = [
        Row {
            case: "bucket, no subdirectory",
            bucket_arn: "arn:aws:s3:::plain-bucket",
            subdirectory: None,
            storage_class: None,
            expected_uri: "s3://plain-bucket/",
            expected_storage_class: "STANDARD",
        },
        Row {
            case: "bucket, leading slash",
            bucket_arn: "arn:aws:s3:::plain-bucket",
            subdirectory: Some("/photos"),
            storage_class: None,
            expected_uri: "s3://plain-bucket/photos/",
            expected_storage_class: "STANDARD",
        },
        Row {
            case: "bucket, trailing slash",
            bucket_arn: "arn:aws:s3:::plain-bucket",
            subdirectory: Some("photos/"),
            storage_class: None,
            expected_uri: "s3://plain-bucket/photos/",
            expected_storage_class: "STANDARD",
        },
        Row {
            case: "bucket, nested prefix",
            bucket_arn: "arn:aws:s3:::plain-bucket",
            subdirectory: Some("/photos/2006/"),
            storage_class: Some("GLACIER"),
            expected_uri: "s3://plain-bucket/photos/2006/",
            expected_storage_class: "GLACIER",
        },
        Row {
            case: "access point",
            bucket_arn: "arn:aws:s3:us-west-2:000000000000:accesspoint/reports-ap",
            subdirectory: Some("/q1"),
            storage_class: None,
            expected_uri: "s3://reports-ap/q1/",
            expected_storage_class: "STANDARD",
        },
        Row {
            // An Outposts bucket is the one form whose documented storage-class
            // default is not STANDARD.
            case: "outposts access point",
            bucket_arn: "arn:aws:s3-outposts:us-west-2:000000000000:outpost/op-1/accesspoint/edge-ap",
            subdirectory: None,
            storage_class: None,
            expected_uri: "s3://edge-ap/",
            expected_storage_class: "OUTPOSTS",
        },
        Row {
            case: "outposts access point, declared class",
            bucket_arn: "arn:aws:s3-outposts:us-west-2:000000000000:outpost/op-1/accesspoint/edge-ap",
            subdirectory: Some("backup"),
            storage_class: Some("STANDARD_IA"),
            expected_uri: "s3://edge-ap/backup/",
            expected_storage_class: "STANDARD_IA",
        },
    ];

    let router = service();
    let mut minted: Vec<String> = Vec::new();

    for row in &rows {
        let mut request = serde_json::json!({
            "S3BucketArn": row.bucket_arn,
            "S3Config": { "BucketAccessRoleArn": "arn:aws:iam::000000000000:role/datasync" },
            "AgentArns": [format!("arn:aws:datasync:{SIGNED_REGION}:{ACCOUNT}:agent/agent-0123456789abcdefg")],
            "Tags": [{ "Key": "team", "Value": "storage" }],
        });
        if let Some(subdirectory) = row.subdirectory {
            request["Subdirectory"] = serde_json::json!(subdirectory);
        }
        if let Some(storage_class) = row.storage_class {
            request["S3StorageClass"] = serde_json::json!(storage_class);
        }

        let created = ok(&router, "CreateLocationS3", request, row.case).await;
        let arn = string(&created, "LocationArn", row.case);
        assert_location_arn(&arn, row.case);
        assert!(
            !minted.contains(&arn),
            "case {:?}: every create mints a fresh ARN, but {arn:?} was minted before",
            row.case
        );
        minted.push(arn.clone());

        let described = ok(
            &router,
            "DescribeLocationS3",
            serde_json::json!({ "LocationArn": arn }),
            row.case,
        )
        .await;
        assert_eq!(
            string(&described, "LocationUri", row.case),
            row.expected_uri,
            "case {:?}: wrong LocationUri, got {described}",
            row.case
        );
        assert_eq!(
            string(&described, "S3StorageClass", row.case),
            row.expected_storage_class,
            "case {:?}: wrong S3StorageClass, got {described}",
            row.case
        );
        assert_eq!(
            described["AgentArns"].as_array().map(Vec::len),
            Some(1),
            "case {:?}: expected the stored AgentArns, got {described}",
            row.case
        );
    }

    // A location ARN that was never minted resolves to nothing.
    invalid_request(
        &router,
        "DescribeLocationS3",
        serde_json::json!({
            "LocationArn": format!("arn:aws:datasync:{SIGNED_REGION}:{ACCOUNT}:location/loc-ffffffffffffffffa"),
        }),
        "unknown location",
    )
    .await;
}

/// `CreateTask` only accepts location ARNs this service holds.
///
/// The rows span which side is unknown, and both.
#[tokio::test]
async fn create_task_refuses_location_arns_that_resolve_to_nothing() {
    let router = service();
    let known = create_bucket_location(&router, "known-bucket", "/in", "setup").await;
    let other_known = create_bucket_location(&router, "other-known-bucket", "/in", "setup").await;
    let unknown =
        format!("arn:aws:datasync:{SIGNED_REGION}:{ACCOUNT}:location/loc-aaaaaaaaaaaaaaaaa");
    let also_unknown =
        format!("arn:aws:datasync:{SIGNED_REGION}:{ACCOUNT}:location/loc-bbbbbbbbbbbbbbbbb");

    for (case, source, destination) in [
        ("unknown source", unknown.clone(), known.clone()),
        ("unknown destination", known.clone(), unknown.clone()),
        ("both unknown", unknown.clone(), also_unknown),
    ] {
        invalid_request(
            &router,
            "CreateTask",
            serde_json::json!({
                "SourceLocationArn": source,
                "DestinationLocationArn": destination,
            }),
            case,
        )
        .await;
    }

    // The same request with both sides known succeeds, so the refusals above are
    // about resolution and nothing else.
    let created = ok(
        &router,
        "CreateTask",
        serde_json::json!({
            "SourceLocationArn": known,
            "DestinationLocationArn": other_known,
        }),
        "both known",
    )
    .await;
    assert_task_arn(&string(&created, "TaskArn", "both known"), "both known");
}

/// `ListTasks` pages over every task in creation order, resuming exactly where the
/// previous page ended and omitting the token on the last page.
///
/// The rows are every page size from 1 to one past the number of tasks, which
/// exhausts the sizes that can split five tasks, plus the modelled maximum.
#[tokio::test]
async fn list_tasks_pages_in_creation_order_without_gaps_or_repeats() {
    let router = service();
    let mut created: Vec<String> = Vec::new();
    for index in 0..5 {
        created.push(create_task(&router, &format!("paged-task-{index}")).await);
    }

    for page_size in [1usize, 2, 3, 4, 5, 6, 100] {
        let case = format!("MaxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;

        loop {
            let mut request = serde_json::json!({ "MaxResults": page_size });
            if let Some(token) = &token {
                request["NextToken"] = serde_json::json!(token);
            }
            let page = ok(&router, "ListTasks", request, &case).await;
            let entries = page["Tasks"].as_array().cloned().unwrap_or_default();
            assert!(
                entries.len() <= page_size,
                "case {case:?}: a page carried {} entries, more than MaxResults",
                entries.len()
            );
            for entry in &entries {
                let arn = entry["TaskArn"].as_str().unwrap_or_default().to_string();
                assert!(
                    !seen.contains(&arn),
                    "case {case:?}: {arn:?} appeared on two pages"
                );
                assert_eq!(
                    entry["TaskMode"].as_str(),
                    Some("BASIC"),
                    "case {case:?}: expected each entry's stored TaskMode, got {entry}"
                );
                assert!(
                    entry["Status"].is_string(),
                    "case {case:?}: expected each entry's stored Status, got {entry}"
                );
                seen.push(arn);
            }
            token = page["NextToken"].as_str().map(ToString::to_string);
            pages += 1;
            assert!(
                pages <= created.len() + 2,
                "case {case:?}: pagination did not terminate after {pages} pages"
            );
            if token.is_none() {
                break;
            }
        }

        assert_eq!(
            seen, created,
            "case {case:?}: paging must yield every task in creation order"
        );
        let expected_pages = created.len().div_ceil(page_size);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages, walked {pages}"
        );
    }

    // A cursor stays valid across a delete: the page after it resumes at the next
    // surviving task rather than repeating or skipping one.
    let first_page = ok(
        &router,
        "ListTasks",
        serde_json::json!({ "MaxResults": 2 }),
        "delete between pages",
    )
    .await;
    let token = string(&first_page, "NextToken", "delete between pages");
    ok(
        &router,
        "DeleteTask",
        serde_json::json!({ "TaskArn": created[3] }),
        "delete between pages",
    )
    .await;
    let second_page = ok(
        &router,
        "ListTasks",
        serde_json::json!({ "MaxResults": 10, "NextToken": token }),
        "delete between pages",
    )
    .await;
    let remaining: Vec<String> = second_page["Tasks"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| entry["TaskArn"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        remaining,
        vec![created[2].clone(), created[4].clone()],
        "case \"delete between pages\": expected the surviving tail, got {second_page}"
    );

    // A token this service never issued is refused rather than silently restarting.
    for bad in ["not-a-token", "ZZZZ", "00"] {
        invalid_request(
            &router,
            "ListTasks",
            serde_json::json!({ "NextToken": bad }),
            bad,
        )
        .await;
    }
}

/// `ListTasks` applies the filters the model declares: `LocationId` against either of a
/// task's locations, and `CreationTime` as an instant.
///
/// The rows span every `Operator` the two filter names can be compared with, including
/// the negated ones and the multi-valued `In`.
#[tokio::test]
async fn list_tasks_applies_modelled_filters() {
    let router = service();
    let alpha = create_bucket_location(&router, "alpha-bucket", "/in", "setup").await;
    let beta = create_bucket_location(&router, "beta-bucket", "/in", "setup").await;
    // A task copies one location onto another, so each task below needs a second side;
    // this one is the side none of the rows filters on.
    let gamma = create_bucket_location(&router, "gamma-bucket", "/in", "setup").await;

    let on_alpha = string(
        &ok(
            &router,
            "CreateTask",
            serde_json::json!({
                "SourceLocationArn": alpha,
                "DestinationLocationArn": gamma,
                "Name": "alpha-only",
            }),
            "setup",
        )
        .await,
        "TaskArn",
        "setup",
    );
    let alpha_to_beta = string(
        &ok(
            &router,
            "CreateTask",
            serde_json::json!({
                "SourceLocationArn": alpha,
                "DestinationLocationArn": beta,
                "Name": "alpha-to-beta",
            }),
            "setup",
        )
        .await,
        "TaskArn",
        "setup",
    );
    let on_beta = string(
        &ok(
            &router,
            "CreateTask",
            serde_json::json!({
                "SourceLocationArn": beta,
                "DestinationLocationArn": gamma,
                "Name": "beta-only",
            }),
            "setup",
        )
        .await,
        "TaskArn",
        "setup",
    );

    // `beta` is the destination of one task and the source of another, so an
    // either-side match is observable.
    let rows: Vec<(&str, serde_json::Value, Vec<String>)> = vec![
        (
            "LocationId Equals alpha",
            serde_json::json!([{ "Name": "LocationId", "Operator": "Equals", "Values": [alpha] }]),
            vec![on_alpha.clone(), alpha_to_beta.clone()],
        ),
        (
            "LocationId Equals beta",
            serde_json::json!([{ "Name": "LocationId", "Operator": "Equals", "Values": [beta] }]),
            vec![alpha_to_beta.clone(), on_beta.clone()],
        ),
        (
            "LocationId In both",
            serde_json::json!([{ "Name": "LocationId", "Operator": "In", "Values": [alpha, beta] }]),
            vec![on_alpha.clone(), alpha_to_beta.clone(), on_beta.clone()],
        ),
        (
            "LocationId NotEquals alpha",
            serde_json::json!([{ "Name": "LocationId", "Operator": "NotEquals", "Values": [alpha] }]),
            vec![on_beta.clone()],
        ),
        (
            "LocationId BeginsWith the ARN prefix",
            serde_json::json!([{
                "Name": "LocationId",
                "Operator": "BeginsWith",
                "Values": [format!("arn:aws:datasync:{SIGNED_REGION}:{ACCOUNT}:location/")],
            }]),
            vec![on_alpha.clone(), alpha_to_beta.clone(), on_beta.clone()],
        ),
        (
            "LocationId Contains a suffix of beta",
            serde_json::json!([{
                "Name": "LocationId",
                "Operator": "Contains",
                "Values": [beta[beta.len() - 6..].to_string()],
            }]),
            vec![alpha_to_beta.clone(), on_beta.clone()],
        ),
        (
            "LocationId NotContains a suffix of beta",
            serde_json::json!([{
                "Name": "LocationId",
                "Operator": "NotContains",
                "Values": [beta[beta.len() - 6..].to_string()],
            }]),
            vec![on_alpha.clone()],
        ),
        (
            "CreationTime GreaterThan the epoch",
            serde_json::json!([{ "Name": "CreationTime", "Operator": "GreaterThan", "Values": ["0"] }]),
            vec![on_alpha.clone(), alpha_to_beta.clone(), on_beta.clone()],
        ),
        (
            "CreationTime LessThan the epoch",
            serde_json::json!([{ "Name": "CreationTime", "Operator": "LessThan", "Values": ["0"] }]),
            vec![],
        ),
        (
            "CreationTime LessThan an RFC 3339 instant far in the future",
            serde_json::json!([{
                "Name": "CreationTime",
                "Operator": "LessThan",
                "Values": ["2999-01-01T00:00:00Z"],
            }]),
            vec![on_alpha.clone(), alpha_to_beta.clone(), on_beta.clone()],
        ),
        (
            "two filters must both hold",
            serde_json::json!([
                { "Name": "LocationId", "Operator": "Equals", "Values": [beta] },
                { "Name": "CreationTime", "Operator": "LessThan", "Values": ["0"] },
            ]),
            vec![],
        ),
    ];

    for (case, filters, expected) in rows {
        let listed = ok(
            &router,
            "ListTasks",
            serde_json::json!({ "Filters": filters }),
            case,
        )
        .await;
        let actual: Vec<String> = listed["Tasks"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|entry| entry["TaskArn"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            actual, expected,
            "case {case:?}: wrong filtered task list, got {listed}"
        );
    }

    // A `CreationTime` value that is not an instant is refused, not ignored.
    invalid_request(
        &router,
        "ListTasks",
        serde_json::json!({
            "Filters": [{ "Name": "CreationTime", "Operator": "Equals", "Values": ["yesterday"] }],
        }),
        "CreationTime that is not an instant",
    )
    .await;
}

/// The task options a request leaves out take the default the model documents, and the
/// ones it declares survive to both `DescribeTask` and the execution's effective
/// options.
///
/// The rows span the two task modes (whose documented `VerifyMode` defaults differ) and
/// declaring none, some and an overriding subset of the options.
#[tokio::test]
async fn task_options_resolve_to_the_modelled_defaults_and_overlays() {
    struct Row {
        case: &'static str,
        task_mode: Option<&'static str>,
        options: serde_json::Value,
        override_options: serde_json::Value,
        expected_task_options: &'static [(&'static str, &'static str)],
        expected_execution_options: &'static [(&'static str, &'static str)],
    }

    let rows = [
        Row {
            case: "no options, default mode",
            task_mode: None,
            options: serde_json::json!(null),
            override_options: serde_json::json!(null),
            // Every value here is the default the member's own documentation names.
            expected_task_options: &[
                ("VerifyMode", "POINT_IN_TIME_CONSISTENT"),
                ("OverwriteMode", "ALWAYS"),
                ("Atime", "BEST_EFFORT"),
                ("Mtime", "PRESERVE"),
                ("Uid", "INT_VALUE"),
                ("Gid", "INT_VALUE"),
                ("PreserveDeletedFiles", "PRESERVE"),
                ("PreserveDevices", "NONE"),
                ("PosixPermissions", "PRESERVE"),
                ("TaskQueueing", "ENABLED"),
                ("LogLevel", "OFF"),
                ("TransferMode", "CHANGED"),
                ("SecurityDescriptorCopyFlags", "OWNER_DACL"),
                ("ObjectTags", "PRESERVE"),
                ("ObjectMetadata", "PRESERVE"),
            ],
            expected_execution_options: &[("VerifyMode", "POINT_IN_TIME_CONSISTENT")],
        },
        Row {
            case: "no options, enhanced mode",
            task_mode: Some("ENHANCED"),
            options: serde_json::json!(null),
            override_options: serde_json::json!(null),
            // The model documents ONLY_FILES_TRANSFERRED as the Enhanced-mode default.
            expected_task_options: &[("VerifyMode", "ONLY_FILES_TRANSFERRED")],
            expected_execution_options: &[("VerifyMode", "ONLY_FILES_TRANSFERRED")],
        },
        Row {
            case: "some options declared",
            task_mode: Some("BASIC"),
            options: serde_json::json!({ "VerifyMode": "NONE", "TransferMode": "ALL" }),
            override_options: serde_json::json!(null),
            expected_task_options: &[
                ("VerifyMode", "NONE"),
                ("TransferMode", "ALL"),
                // Untouched members still take their documented default.
                ("Atime", "BEST_EFFORT"),
            ],
            expected_execution_options: &[("VerifyMode", "NONE"), ("TransferMode", "ALL")],
        },
        Row {
            case: "execution overrides a subset",
            task_mode: Some("BASIC"),
            options: serde_json::json!({ "VerifyMode": "NONE", "LogLevel": "BASIC" }),
            override_options: serde_json::json!({ "LogLevel": "TRANSFER" }),
            expected_task_options: &[("VerifyMode", "NONE"), ("LogLevel", "BASIC")],
            expected_execution_options: &[
                // Only the overridden member changes for this run.
                ("LogLevel", "TRANSFER"),
                ("VerifyMode", "NONE"),
            ],
        },
    ];

    let router = service();
    let location = create_bucket_location(&router, "options-bucket", "/in", "setup").await;
    let destination =
        create_bucket_location(&router, "options-destination-bucket", "/out", "setup").await;

    for (index, row) in rows.iter().enumerate() {
        let mut request = serde_json::json!({
            "SourceLocationArn": location,
            "DestinationLocationArn": destination,
            "Name": format!("options-row-{index}"),
        });
        if let Some(task_mode) = row.task_mode {
            request["TaskMode"] = serde_json::json!(task_mode);
        }
        if !row.options.is_null() {
            request["Options"] = row.options.clone();
        }

        let task_arn = string(
            &ok(&router, "CreateTask", request, row.case).await,
            "TaskArn",
            row.case,
        );
        let described = ok(
            &router,
            "DescribeTask",
            serde_json::json!({ "TaskArn": task_arn }),
            row.case,
        )
        .await;
        for (member, expected) in row.expected_task_options {
            assert_eq!(
                described["Options"][member].as_str(),
                Some(*expected),
                "case {:?}: wrong task Options.{member}, got {described}",
                row.case
            );
        }
        assert_eq!(
            described["Options"]["BytesPerSecond"].as_i64(),
            Some(-1),
            "case {:?}: expected the no-limit BytesPerSecond sentinel, got {described}",
            row.case
        );

        std::thread::sleep(ONE_SPAN);
        let mut start = serde_json::json!({ "TaskArn": task_arn });
        if !row.override_options.is_null() {
            start["OverrideOptions"] = row.override_options.clone();
        }
        let execution_arn = string(
            &ok(&router, "StartTaskExecution", start, row.case).await,
            "TaskExecutionArn",
            row.case,
        );
        let execution = ok(
            &router,
            "DescribeTaskExecution",
            serde_json::json!({ "TaskExecutionArn": execution_arn }),
            row.case,
        )
        .await;
        for (member, expected) in row.expected_execution_options {
            assert_eq!(
                execution["Options"][member].as_str(),
                Some(*expected),
                "case {:?}: wrong effective Options.{member}, got {execution}",
                row.case
            );
        }
        assert_eq!(
            execution["TaskMode"].as_str(),
            Some(row.task_mode.unwrap_or("BASIC")),
            "case {:?}: an execution reports its task's mode, got {execution}",
            row.case
        );
    }
}

/// An execution's phase, timestamps and per-phase durations all come from the schedule
/// stored when it started, so they only ever move forward and are the same for every
/// observer.
#[tokio::test]
async fn task_execution_phases_advance_only_with_the_clock() {
    let case = "phases";
    let router = service();
    let task_arn = create_task(&router, "phase-task").await;
    std::thread::sleep(ONE_SPAN);
    let execution_arn = string(
        &ok(
            &router,
            "StartTaskExecution",
            serde_json::json!({ "TaskArn": task_arn }),
            case,
        )
        .await,
        "TaskExecutionArn",
        case,
    );
    let describe = serde_json::json!({ "TaskExecutionArn": execution_arn });

    // The modelled phase order. Two reads at the same instant must agree, and a
    // later read must never report an earlier phase.
    let order = [
        "LAUNCHING",
        "PREPARING",
        "TRANSFERRING",
        "VERIFYING",
        "SUCCESS",
    ];
    let rank = |status: &str| {
        order
            .iter()
            .position(|known| *known == status)
            .unwrap_or_else(|| panic!("case {case:?}: {status:?} is not a modelled phase"))
    };

    let mut highest = 0;
    let mut reached_success = false;
    for step in 0..12 {
        let first = ok(&router, "DescribeTaskExecution", describe.clone(), case).await;
        let second = ok(&router, "DescribeTaskExecution", describe.clone(), case).await;
        let status = string(&first, "Status", case);
        assert_eq!(
            status,
            string(&second, "Status", case),
            "case {case:?}: two reads at step {step} disagreed, got {first} then {second}"
        );
        assert_eq!(
            first["StartTime"], second["StartTime"],
            "case {case:?}: StartTime changed between two reads at step {step}"
        );
        let now = rank(&status);
        assert!(
            now >= highest,
            "case {case:?}: phase went backwards at step {step}: {status:?} after rank {highest}"
        );
        highest = now;
        if status == "SUCCESS" {
            reached_success = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(
        reached_success,
        "case {case:?}: the execution never reached SUCCESS; highest phase rank was {highest}"
    );

    // Once ended, the execution is stable: the terminal body does not drift.
    let ended = ok(&router, "DescribeTaskExecution", describe.clone(), case).await;
    std::thread::sleep(Duration::from_millis(400));
    let still_ended = ok(&router, "DescribeTaskExecution", describe, case).await;
    assert_eq!(
        ended, still_ended,
        "case {case:?}: a finished execution must report the same body on every read"
    );
}
