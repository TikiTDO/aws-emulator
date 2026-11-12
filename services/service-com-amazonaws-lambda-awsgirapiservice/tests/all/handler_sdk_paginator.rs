// SPDX-License-Identifier: CC0-1.0

//! The PAGINATOR of the AWS SDK a running Python handler imports, end to end over the wire:
//! Lambda beside the service being listed on ONE engine, a `python3.x` function whose code
//! asks its client for a paginator, and the pages that paginator served.
//!
//! RULE: a managed runtime's SDK gives a handler a paginator for every operation the called
//! service's own model states pagination for — `client.get_paginator(<operation>)` pages that
//! operation through by sending the continuation value each answer carries back in the input
//! member the model names, one call per page, until an answer carries none, and refuses an
//! operation the model states no pagination for; an operation's other input members are
//! carried on every page, a `PaginationConfig` bounds the page size and how many items the
//! iterator serves, and `build_full_result` joins the pages' own result members.
//!
//! The rows differ along what the rule generalises over: WHICH service and operation is paged
//! (so which members the model names for it), what the continuation value IS — text for an
//! object listing, a whole item key for a table scan — whether the model names the paged
//! items itself or the page's results are the output's own list members, how the caller bounds
//! the iteration (a page size, a cap on the items served, a resume token), and whether the
//! operation is paginated at all.
//!
//! Each assertion reads what the GUEST was served, and the listings themselves are built by
//! writing through each service's own API first, so a paginator that served a page nothing
//! answered, dropped a page, or repeated one fails here.
//!
//! The suites RUN handlers, so they take the target's one invocation stream
//! ([`crate::invoke_request_id::one_invocation_stream_at_a_time`]): this emulator serves at
//! most `max_concurrent_runs` invocations at once for the whole process.

use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

// The one .zip writer this target packages a running handler with.
use crate::synchronous_invoke_run::package;

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The `X-Amz-Target` prefix of the DynamoDB service (its `OPERATIONS.md`, "Wire protocol").
const DYNAMODB_TARGET_PREFIX: &str = "DynamoDB_20120810";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The module and attribute every package below carries its handler as.
const HANDLER: &str = "lambda_function.lambda_handler";

/// The runtime every function below is created with.
const RUNTIME: &str = "python3.12";

/// How long one invocation is given, as the function's own `Timeout`: well past what a run
/// needs, because what is under test is the SDK's interface and not how fast an interpreter
/// gets through a handler.
const FUNCTION_TIMEOUT: u64 = 30;

/// How long past a due instant these suites wait before calling. The transition is owed by the
/// CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(150);

/// How long a wait for a table to settle is given before the test is a failure.
const BUDGET: Duration = Duration::from_secs(30);

/// How long between two readings of a resource that is settling.
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// Lambda, Amazon S3 and Amazon DynamoDB over ONE shared engine, as `core/router/src/lib.rs`
/// mounts them: a handler's SDK call is dispatched back into this same router, so what it
/// lists is what this suite wrote.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(Arc::clone(&engine)),
    );
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature is never checked here.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One Amazon S3 call made as a CLIENT makes it — path-style, signed for `s3` — asserted to
/// have answered 200.
async fn s3(router: &MultiServiceRouter, case: &str, method: &str, uri: &str, body: &[u8]) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header("authorization", authorization("s3"))
        .body(bytes::Bytes::from(body.to_vec()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let answered = response.into_body();
    assert_eq!(
        status,
        200,
        "case {case:?}: expected HTTP 200 from S3 {method} {uri}, got {status} ({})",
        String::from_utf8_lossy(&answered)
    );
}

/// One DynamoDB call (`awsJson1_0`, `DynamoDB_20120810.<Operation>`) that must answer 200.
async fn dynamodb(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("dynamodb.{REGION}.amazonaws.com"))
        .header("authorization", authorization("dynamodb"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{DYNAMODB_TARGET_PREFIX}.{operation}"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed: Value = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DynamoDB {operation}, got {status} ({parsed})"
    );
    parsed
}

/// One Lambda `restJson1` call, as its status, headers and parsed body.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &[u8],
) -> (u16, http::HeaderMap, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("authorization", authorization("lambda"))
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(body.to_vec()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    let parsed: Value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).to_string()))
    };
    (parts.status.as_u16(), parts.headers, parsed)
}

/// Create a function running `source` and wait out the provisioning window.
async fn given_function(router: &MultiServiceRouter, case: &str, name: &str, source: &str) {
    let (status, _headers, created) = lambda(
        router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": RUNTIME,
            "Handler": HANDLER,
            "Timeout": FUNCTION_TIMEOUT,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(source)),
            },
        }))
        .expect("the create body serializes"),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201 from CreateFunction, got {status} ({created})"
    );
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;
}

/// `Invoke` the function synchronously with `event`, asserting it ran to completion, and
/// answer the payload the run produced.
async fn invoke(router: &MultiServiceRouter, case: &str, name: &str, event: &Value) -> Value {
    let (status, headers, payload) = lambda(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/invocations"),
        event.to_string().as_bytes(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Invoke, got {status} ({payload})"
    );
    let signal = headers
        .get("x-amz-function-error")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    assert_eq!(
        signal, None,
        "case {case:?}: the handler must run to completion, got the function error \
         {signal:?} with {payload}"
    );
    payload
}

/// The bucket the object-listing rows run over.
const BUCKET: &str = "paginated-archive";

/// The prefix the listing is scoped to, and the keys under it — more than one page's worth, so
/// the listing really is paged.
const PREFIX: &str = "ledger/";

/// The keys under [`PREFIX`], in the byte order a listing reports them in.
const SCOPED_KEYS: &[&str] = &[
    "ledger/a.txt",
    "ledger/b.txt",
    "ledger/c.txt",
    "ledger/d.txt",
    "ledger/e.txt",
];

/// Keys OUTSIDE that prefix, sorting after it: a page that dropped the request's own `Prefix`
/// would report them.
const UNSCOPED_KEYS: &[&str] = &["other/y.txt", "other/z.txt"];

/// The handler the object-listing suite runs: every way a caller drives one paginator,
/// each reported under its own name.
const LISTING_HANDLER_SOURCE: &str = r#"
import boto3
import botocore.exceptions

CLIENT = boto3.client("s3")


def lambda_handler(event, context):
    bucket = event["bucket"]
    prefix = event["prefix"]
    paging = {"PageSize": event["page_size"]}
    paginator = CLIENT.get_paginator("list_objects_v2")
    steps = {}

    # Every page of the listing, each page's own keys apart, so what is reported is how the
    # listing was cut into pages — and whether every page carried the request's own prefix.
    steps["pages"] = [
        [one["Key"] for one in page.get("Contents", [])]
        for page in paginator.paginate(Bucket=bucket, Prefix=prefix, PaginationConfig=paging)
    ]

    # The pages joined into the one document the SDK joins them into.
    joined = paginator.paginate(
        Bucket=bucket, Prefix=prefix, PaginationConfig=paging
    ).build_full_result()
    steps["joined"] = {
        "keys": [one["Key"] for one in joined.get("Contents", [])],
        "resumed_at": joined.get("NextToken"),
    }

    # Capping how many items the iterator serves, and resuming at the token it stopped at:
    # one row per cap, so a cap falling inside a page and one falling on a page's edge are
    # both reported.
    steps["capped"] = []
    for cap in event["caps"]:
        capped = paginator.paginate(
            Bucket=bucket,
            Prefix=prefix,
            PaginationConfig={"PageSize": event["page_size"], "MaxItems": cap},
        )
        served = [one["Key"] for page in capped for one in page.get("Contents", [])]
        # Resuming at that token serves what is left, and nothing twice.
        rest = [
            one["Key"]
            for page in paginator.paginate(
                Bucket=bucket,
                Prefix=prefix,
                PaginationConfig={
                    "PageSize": event["page_size"],
                    "StartingToken": capped.resume_token,
                },
            )
            for one in page.get("Contents", [])
        ]
        steps["capped"].append({"cap": cap, "served": served, "rest": rest})

    # Which operations have a paginator at all, and what asking for one that has none says.
    steps["pageable"] = {
        "list_objects_v2": CLIENT.can_paginate("list_objects_v2"),
        "get_object": CLIENT.can_paginate("get_object"),
    }
    try:
        CLIENT.get_paginator("get_object")
    except botocore.exceptions.OperationNotPageableError as error:
        steps["refused"] = {"type": type(error).__name__, "message": str(error)}
    else:
        steps["refused"] = None
    return steps
"#;

/// One row of the listing table: what the caller asked the paginator for, and what it served.
struct Row {
    case: &'static str,
    /// The step the handler reported it under.
    step: &'static str,
    /// What the guest must have been served.
    expected: Value,
}

/// A listing longer than one page, paged through by the paginator the runtime's SDK gives the
/// handler: the pages themselves, the pages joined, iterations capped at each place a cap can
/// fall with the resumption of each, and the operations that have no paginator at all.
#[tokio::test]
async fn a_handler_pages_an_object_listing_through_the_sdks_paginator() {
    let _stream = crate::invoke_request_id::one_invocation_stream_at_a_time().await;
    let case = "an object listing paged through the SDK's paginator";
    let router = router();

    s3(&router, case, "PUT", &format!("/{BUCKET}"), b"").await;
    for key in SCOPED_KEYS.iter().chain(UNSCOPED_KEYS) {
        s3(&router, case, "PUT", &format!("/{BUCKET}/{key}"), key.as_bytes()).await;
    }

    let caller = "listing-paginator-caller";
    given_function(&router, case, caller, LISTING_HANDLER_SOURCE).await;
    let served = invoke(
        &router,
        case,
        caller,
        &json!({ "bucket": BUCKET, "prefix": PREFIX, "page_size": 2, "caps": [1, 3, 4] }),
    )
    .await;

    let rows = vec![
        Row {
            case: "the pages a page size of two cuts five keys into",
            step: "pages",
            expected: json!([
                [SCOPED_KEYS[0], SCOPED_KEYS[1]],
                [SCOPED_KEYS[2], SCOPED_KEYS[3]],
                [SCOPED_KEYS[4]],
            ]),
        },
        Row {
            case: "the pages joined into one document, which reached the end",
            step: "joined",
            expected: json!({ "keys": SCOPED_KEYS, "resumed_at": Value::Null }),
        },
        Row {
            case: "capped iterations and the resumption at the token each stopped at",
            step: "capped",
            expected: json!([
                // A cap inside the FIRST page: it is truncated, and the resumption drops
                // exactly what was already served out of the page it asks for again.
                {
                    "cap": 1,
                    "served": [SCOPED_KEYS[0]],
                    "rest": [SCOPED_KEYS[1], SCOPED_KEYS[2], SCOPED_KEYS[3], SCOPED_KEYS[4]],
                },
                // A cap inside a LATER page.
                {
                    "cap": 3,
                    "served": [SCOPED_KEYS[0], SCOPED_KEYS[1], SCOPED_KEYS[2]],
                    "rest": [SCOPED_KEYS[3], SCOPED_KEYS[4]],
                },
                // A cap falling exactly on a page's edge, where the token is the next
                // page's own and nothing is truncated.
                {
                    "cap": 4,
                    "served": [
                        SCOPED_KEYS[0],
                        SCOPED_KEYS[1],
                        SCOPED_KEYS[2],
                        SCOPED_KEYS[3],
                    ],
                    "rest": [SCOPED_KEYS[4]],
                },
            ]),
        },
        Row {
            case: "which operations the service's model states pagination for",
            step: "pageable",
            expected: json!({ "list_objects_v2": true, "get_object": false }),
        },
        Row {
            case: "asking for the paginator of an operation that has none",
            step: "refused",
            expected: json!({
                "type": "OperationNotPageableError",
                "message": "Operation cannot be paginated: get_object",
            }),
        },
    ];
    for row in &rows {
        let actual = served[row.step].clone();
        assert_eq!(
            actual, row.expected,
            "case {:?}: expected {}, got {actual} (the whole run: {served})",
            row.case, row.expected
        );
    }
}

/// The table the scan rows run over.
const TABLE: &str = "paginated-ledger";

/// The table's partition and sort key attributes.
const PARTITION_KEY: &str = "pk";
/// See [`PARTITION_KEY`].
const SORT_KEY: &str = "sk";

/// The one partition every item below is written under, so one scan page's continuation value
/// is a whole item key rather than text.
const PARTITION: &str = "ledger";

/// The sort keys of the items the scan reads.
const SORT_KEYS: &[&str] = &["s0", "s1", "s2", "s3", "s4"];

/// The handler the scan suite runs: the same paginator interface over an operation whose
/// continuation value is a whole item key and whose model names the paged items itself.
const SCAN_HANDLER_SOURCE: &str = r#"
import boto3

CLIENT = boto3.client("dynamodb")


def lambda_handler(event, context):
    paging = {"PageSize": event["page_size"]}
    paginator = CLIENT.get_paginator("scan")
    pages = [
        [item[event["sort_key"]]["S"] for item in page.get("Items", [])]
        for page in paginator.paginate(TableName=event["table"], PaginationConfig=paging)
    ]
    joined = paginator.paginate(
        TableName=event["table"], PaginationConfig=paging
    ).build_full_result()
    return {
        "page_sizes": [len(page) for page in pages],
        "sort_keys": sorted(key for page in pages for key in page),
        "joined_sort_keys": sorted(
            item[event["sort_key"]]["S"] for item in joined.get("Items", [])
        ),
        "pageable": {
            "scan": CLIENT.can_paginate("scan"),
            "put_item": CLIENT.can_paginate("put_item"),
        },
    }
"#;

/// The rule holds for an operation whose continuation value is a whole item KEY and whose
/// model names the paged items itself: the same paginator pages a table scan through, serving
/// every item exactly once across the pages a page size cuts it into.
#[tokio::test]
async fn a_handler_pages_a_table_scan_through_the_same_paginator() {
    let _stream = crate::invoke_request_id::one_invocation_stream_at_a_time().await;
    let case = "a table scan paged through the SDK's paginator";
    let router = router();

    dynamodb(
        &router,
        case,
        "CreateTable",
        &json!({
            "TableName": TABLE,
            "KeySchema": [
                { "AttributeName": PARTITION_KEY, "KeyType": "HASH" },
                { "AttributeName": SORT_KEY, "KeyType": "RANGE" },
            ],
            "AttributeDefinitions": [
                { "AttributeName": PARTITION_KEY, "AttributeType": "S" },
                { "AttributeName": SORT_KEY, "AttributeType": "S" },
            ],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    let deadline = Instant::now() + BUDGET;
    loop {
        let described = dynamodb(&router, case, "DescribeTable", &json!({ "TableName": TABLE })).await;
        if described["Table"]["TableStatus"] == json!("ACTIVE") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the table never became ACTIVE: {described}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
    for sort_key in SORT_KEYS {
        dynamodb(
            &router,
            case,
            "PutItem",
            &json!({
                "TableName": TABLE,
                "Item": {
                    PARTITION_KEY: { "S": PARTITION },
                    SORT_KEY: { "S": sort_key },
                },
            }),
        )
        .await;
    }

    let caller = "scan-paginator-caller";
    given_function(&router, case, caller, SCAN_HANDLER_SOURCE).await;
    let served = invoke(
        &router,
        case,
        caller,
        &json!({ "table": TABLE, "sort_key": SORT_KEY, "page_size": 2 }),
    )
    .await;

    assert_eq!(
        served,
        json!({
            // Five items at two per page: the last page is the short one, and a scan that
            // was never continued would be one page of two.
            "page_sizes": [2, 2, 1],
            "sort_keys": SORT_KEYS,
            "joined_sort_keys": SORT_KEYS,
            "pageable": { "scan": true, "put_item": false },
        }),
        "case {case:?}: expected the scan paged two items at a time, got {served}"
    );
}
