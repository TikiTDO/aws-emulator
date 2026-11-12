// SPDX-License-Identifier: CC0-1.0

//! A Resilience Hub resource mapping names a physical resource that REALLY EXISTS —
//! as the service that owns that resource says.
//!
//! The rule under test: **`CreateAppVersionResource` resolves the
//! `physicalResourceId` it is handed against the service owning a resource of the
//! stated `resourceType`, and refuses the mapping with the modelled 400
//! `ValidationException` when that service answers that it holds no such resource;
//! a mapping no owning service answered for is accepted.** Everything built on the
//! app template afterwards — a resolution, an assessment, a resiliency score — is
//! about the resources the template maps, so a mapping of a resource that is not
//! there would put a resource under assessment that nothing holds.
//!
//! Two crates have to agree for that to hold (Resilience Hub asks, the owning
//! service answers), so the suite lives here: a crate-local test can only ever see
//! one side of it.
//!
//! The rows differ along what the rule generalises over: the resource CLASS (an
//! object store bucket, a queue, a topic, a table), the FORM the mapping names the
//! resource by (a bare name, a queue url, an ARN), whether the resource was created
//! at all, and a class no owner here is asked about — which is accepted, because
//! nothing answered that it is missing.
//!
//! The publish half is the same rule's consequence: a draft whose only mapping was
//! refused maps no resource, so `PublishAppVersion` refuses to freeze it.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for, and so the region each resource
/// and each ARN belongs to.
const REGION: &str = "us-east-1";

/// The account id every ARN and queue url this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// The refusal a publish of a draft whose template maps no resource earns.
const EMPTY_TEMPLATE_MESSAGE: &str =
    "Invalid app template. App template must have at least 1 resource";

/// The services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them: Resilience Hub plus the owners of the
/// resource classes it asks about.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signing is not the subject here, so requests are unauthenticated; the
    // credential scope is still carried, because it is what names the Region and
    // routes the request.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_resiliencehub_awsresiliencehub::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name`, which
/// is what routes the request. The signature is never checked.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=00"
    )
}

/// What a dispatched request answered: the status, the `x-amzn-errortype` code (empty
/// on a success) and the parsed body.
struct Answer {
    status: u16,
    code: String,
    body: Value,
}

impl Answer {
    /// The body, asserting the call succeeded.
    fn ok(&self, case: &str) -> &Value {
        assert_eq!(
            self.status, 200,
            "case {case:?}: expected HTTP 200, got {} {:?} {:?}",
            self.status, self.code, self.body
        );
        &self.body
    }

    /// The `message` the answer carried.
    fn message(&self) -> String {
        self.body["message"].as_str().unwrap_or_default().to_string()
    }
}

/// One Resilience Hub restJson1 call: `POST <path>` with a JSON body.
async fn resiliencehub(router: &MultiServiceRouter, path: &str, body: &Value) -> Answer {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://resiliencehub.{REGION}.amazonaws.com{path}"))
        .header("host", format!("resiliencehub.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("resiliencehub"))
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let body = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    Answer { status, code, body }
}

/// One `awsJson` RPC call, requiring a 200: `POST /` carrying
/// `X-Amz-Target: <target_prefix>.<operation>`.
async fn rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", format!("application/x-amz-json-{json_version}"))
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header("authorization", authorization(signing_name))
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "setup: {signing_name} {operation} must succeed, got {status} with {parsed}"
    );
    parsed
}

/// `PUT /<bucket>` — Amazon S3's `CreateBucket`.
///
/// A create in us-east-1 carries NO location constraint: that Region is the
/// unspecified constraint, and S3 refuses a create that names it
/// (`InvalidLocationConstraint`).
async fn create_bucket(router: &MultiServiceRouter, bucket: &str) {
    let request = http::Request::builder()
        .method("PUT")
        .uri(format!("/{bucket}"))
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header("authorization", authorization("s3"))
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    assert_eq!(
        response.status().as_u16(),
        200,
        "setup: CreateBucket({bucket:?}) must succeed, got {} with {:?}",
        response.status(),
        String::from_utf8_lossy(response.body())
    );
}

/// Create a queue and answer its url.
async fn create_queue(router: &MultiServiceRouter, name: &str) -> String {
    let created = rpc(
        router,
        "sqs",
        "AmazonSQS",
        "1.0",
        "CreateQueue",
        &json!({"QueueName": name}),
    )
    .await;
    created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: CreateQueue({name:?}) answered {created}"))
        .to_string()
}

/// Create a topic and answer its ARN. Amazon SNS speaks `awsQuery`, so the request
/// is a form body.
async fn create_topic(router: &MultiServiceRouter, name: &str) -> String {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sns.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("sns"))
        .body(Bytes::from(format!(
            "Action=CreateTopic&Version=2010-03-31&Name={name}"
        )))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "setup: CreateTopic({name:?}) must succeed, got {status} with {text}"
    );
    let opened = text
        .find("<TopicArn>")
        .map(|at| at + "<TopicArn>".len())
        .unwrap_or_else(|| panic!("setup: CreateTopic({name:?}) answered {text}"));
    let closed = text[opened..]
        .find("</TopicArn>")
        .unwrap_or_else(|| panic!("setup: CreateTopic({name:?}) answered {text}"));
    text[opened..opened + closed].to_string()
}

/// Create a table with one hash key.
async fn create_table(router: &MultiServiceRouter, name: &str) {
    rpc(
        router,
        "dynamodb",
        "DynamoDB_20120810",
        "1.0",
        "CreateTable",
        &json!({
            "TableName": name,
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
}

/// Create an application and answer its ARN.
async fn create_an_app(router: &MultiServiceRouter, name: &str) -> String {
    let created = resiliencehub(router, "/create-app", &json!({"name": name})).await;
    created.ok(name)["app"]["appArn"]
        .as_str()
        .expect("a minted appArn")
        .to_string()
}

/// Map one resource into `app_arn`'s draft version.
async fn map_a_resource(
    router: &MultiServiceRouter,
    app_arn: &str,
    logical: &str,
    resource_type: &str,
    physical: &str,
) -> Answer {
    resiliencehub(
        router,
        "/create-app-version-resource",
        &json!({
            "appArn": app_arn,
            "logicalResourceId": {"identifier": logical},
            "physicalResourceId": physical,
            "resourceType": resource_type,
            "appComponents": ["appcommon"],
        }),
    )
    .await
}

/// One row of the mapping table.
struct Mapping {
    label: &'static str,
    resource_type: &'static str,
    /// The `physicalResourceId` the mapping names.
    physical: String,
    /// Whether the service owning this resource class holds it.
    exists: bool,
}

/// The whole table: a mapping of a resource its owner holds is stored and reads
/// back, and a mapping of one its owner says it does not hold is the modelled 400
/// naming the type and the identifier — while a class no owner here is asked about
/// is accepted.
#[tokio::test]
async fn a_mapping_is_refused_only_where_the_owning_service_holds_no_such_resource() {
    let router = build_router();

    create_bucket(&router, "rh-mapped-bucket").await;
    let queue_url = create_queue(&router, "rh-mapped-queue").await;
    let topic_arn = create_topic(&router, "rh-mapped-topic").await;
    create_table(&router, "RhMappedTable").await;

    let app_arn = create_an_app(&router, "resource-mapping-existence").await;

    let rows = vec![
        Mapping {
            label: "a bucket the object store holds, by name",
            resource_type: "AWS::S3::Bucket",
            physical: "rh-mapped-bucket".to_string(),
            exists: true,
        },
        Mapping {
            label: "a bucket the object store holds, by ARN",
            resource_type: "AWS::S3::Bucket",
            physical: "arn:aws:s3:::rh-mapped-bucket".to_string(),
            exists: true,
        },
        Mapping {
            label: "a bucket nothing created",
            resource_type: "AWS::S3::Bucket",
            physical: "rh-ghost-bucket".to_string(),
            exists: false,
        },
        Mapping {
            label: "a queue the queue service holds, by name",
            resource_type: "AWS::SQS::Queue",
            physical: "rh-mapped-queue".to_string(),
            exists: true,
        },
        Mapping {
            label: "a queue the queue service holds, by url",
            resource_type: "AWS::SQS::Queue",
            physical: queue_url.clone(),
            exists: true,
        },
        Mapping {
            label: "a queue the queue service holds, by ARN",
            resource_type: "AWS::SQS::Queue",
            physical: format!("arn:aws:sqs:{REGION}:{ACCOUNT}:rh-mapped-queue"),
            exists: true,
        },
        Mapping {
            label: "a queue nothing created",
            resource_type: "AWS::SQS::Queue",
            physical: "rh-ghost-queue".to_string(),
            exists: false,
        },
        Mapping {
            label: "a topic the notification service holds, by ARN",
            resource_type: "AWS::SNS::Topic",
            physical: topic_arn.clone(),
            exists: true,
        },
        Mapping {
            label: "a topic nothing created",
            resource_type: "AWS::SNS::Topic",
            physical: format!("arn:aws:sns:{REGION}:{ACCOUNT}:rh-ghost-topic"),
            exists: false,
        },
        Mapping {
            label: "a table the database holds, by name",
            resource_type: "AWS::DynamoDB::Table",
            physical: "RhMappedTable".to_string(),
            exists: true,
        },
        Mapping {
            label: "a table the database holds, by ARN",
            resource_type: "AWS::DynamoDB::Table",
            physical: format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/RhMappedTable"),
            exists: true,
        },
        Mapping {
            label: "a table nothing created",
            resource_type: "AWS::DynamoDB::Table",
            physical: "RhGhostTable".to_string(),
            exists: false,
        },
        Mapping {
            // Nothing here is asked about an instance, so nothing answered that it
            // is missing: the mapping stands.
            label: "a resource class no owner here is asked about",
            resource_type: "AWS::EC2::Instance",
            physical: "i-0123456789abcdef0".to_string(),
            exists: true,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.label;
        let logical = format!("Mapped{index}");
        let mapped = map_a_resource(
            &router,
            &app_arn,
            &logical,
            row.resource_type,
            &row.physical,
        )
        .await;

        if row.exists {
            let body = mapped.ok(case);
            assert_eq!(
                body["physicalResource"]["physicalResourceId"]["identifier"].as_str(),
                Some(row.physical.as_str()),
                "case {case:?}: the mapping must be stored under the identifier the request \
                 named, got {body:?}"
            );
        } else {
            assert_eq!(
                (mapped.status, mapped.code.as_str()),
                (400, "ValidationException"),
                "case {case:?}: expected the modelled 400 ValidationException, got {} {:?} \
                 {:?}",
                mapped.status,
                mapped.code,
                mapped.body
            );
            let expected = format!(
                "Invalid resource mapping. The given resource does not exist. Resource Type: \
                 {}, Resource Id: {}",
                row.resource_type, row.physical
            );
            let actual = mapped.message();
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }

        // The draft holds exactly the mappings that were accepted: a refused one
        // persisted nothing.
        let read = resiliencehub(
            &router,
            "/describe-app-version-resource",
            &json!({
                "appArn": app_arn,
                "appVersion": "draft",
                "logicalResourceId": {"identifier": logical},
            }),
        )
        .await;
        assert_eq!(
            read.status == 200,
            row.exists,
            "case {case:?}: the draft must hold this resource only if the mapping was \
             accepted, got {} {:?}",
            read.status,
            read.body
        );
    }
}

/// A draft whose every mapping was refused maps no resource, so publishing it is
/// refused too — and mapping one resource that really exists is what makes the same
/// publish succeed.
#[tokio::test]
async fn a_draft_whose_mappings_were_refused_cannot_be_published() {
    let router = build_router();
    let app_arn = create_an_app(&router, "refused-mappings-publish").await;

    // A component, so the draft is not empty — and a mapping of a bucket the object
    // store does not hold, which is refused.
    resiliencehub(
        &router,
        "/create-app-version-app-component",
        &json!({
            "appArn": app_arn,
            "id": "storage-tier",
            "name": "storage-tier",
            "type": "AWS::ResilienceHub::StorageAppComponent",
        }),
    )
    .await
    .ok("the component");
    let refused = map_a_resource(
        &router,
        &app_arn,
        "GhostResource",
        "AWS::S3::Bucket",
        "rh-publish-ghost-bucket",
    )
    .await;
    assert_eq!(
        (refused.status, refused.code.as_str()),
        (400, "ValidationException"),
        "the ghost mapping must be refused, got {} {:?} {:?}",
        refused.status,
        refused.code,
        refused.body
    );

    let unpublishable = resiliencehub(
        &router,
        "/publish-app-version",
        &json!({"appArn": app_arn}),
    )
    .await;
    assert_eq!(
        (unpublishable.status, unpublishable.code.as_str()),
        (409, "ConflictException"),
        "a draft mapping no resource must be refused with the modelled 409, got {} {:?} {:?}",
        unpublishable.status,
        unpublishable.code,
        unpublishable.body
    );
    let actual = unpublishable.message();
    assert_eq!(
        actual, EMPTY_TEMPLATE_MESSAGE,
        "expected {EMPTY_TEMPLATE_MESSAGE:?}, got {actual:?}"
    );

    // The same publish succeeds once a resource the object store really holds is
    // mapped.
    create_bucket(&router, "rh-publishable-bucket").await;
    map_a_resource(
        &router,
        &app_arn,
        "RealResource",
        "AWS::S3::Bucket",
        "rh-publishable-bucket",
    )
    .await
    .ok("the real mapping");
    let published = resiliencehub(
        &router,
        "/publish-app-version",
        &json!({"appArn": app_arn}),
    )
    .await;
    published.ok("the publish");
}
