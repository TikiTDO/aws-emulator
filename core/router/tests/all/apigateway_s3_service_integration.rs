// SPDX-License-Identifier: CC0-1.0

//! An API Gateway stage whose method integrates an AWS SERVICE ACTION: the whole path a
//! client takes from the stage invoke URL to the object in Amazon S3, across three
//! services on one engine.
//!
//! RULE: a deployed method whose integration is of type `AWS` is served by MAKING the call
//! its `uri` names against the service that `uri` names — `Integration$uri` gives the form
//! `arn:aws:apigateway:{region}:{subdomain}:path|action/{service_api}` — under the role
//! `Integration$credentials` names, and the method response is the answer that service
//! gave, mapped through the integration response whose `selectionPattern` matches the
//! status it answered with ("For all other HTTP and AWS back ends, the HTTP status code is
//! matched", `IntegrationResponse$selectionPattern`). The role decides whether the call is
//! made at all: a role whose trust policy does not let `apigateway.amazonaws.com` assume it
//! is an integration that could not be performed (`API_CONFIGURATION_ERROR`, HTTP 500 and
//! `{"message":"Internal server error"}`), and a call the policies in force on the role do
//! not allow is refused by the service with its own `AccessDenied` (HTTP 403), which the
//! integration responses then map like any other backend answer.
//!
//! Only a cross-service suite can pin this: the object is Amazon S3's, the role and its
//! policies are IAM's, and the api, the stage and the integration are API Gateway's, so the
//! rule is observable only where all three are mounted on one engine. Each crate's own
//! tests reach exactly one of them.
//!
//! The rows vary what the rule generalises over: the FORM the `uri` names the call in (the
//! `path` form and the `action` form), the ACTION (read, write, head, delete — the verb
//! names it in the path form), the shape of the KEY (a plain name, one under a prefix
//! reached through a greedy path parameter), the media type of the stored bytes (text and
//! bytes that are not text at all), the status the service answers (an object that is
//! there, a key nothing is stored under), and WHICH role the integration names (one that
//! may make the call, one whose grant names another bucket, one this service cannot assume,
//! and no role at all).

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request here is signed for, and the Region the integration ARNs name.
const REGION: &str = "us-east-1";

/// The long-term key every emulator publishes, which is what the setup calls are signed
/// with. The signature itself is never checked: this router runs with request
/// authentication off, as every suite in this crate runs it.
const AMBIENT_KEY: &str = "AKIAIOSFODNN7EXAMPLE";

/// The bucket every object below is stored in.
const BUCKET: &str = "stage-integration-shelf";

/// The stage the api is deployed to.
const STAGE: &str = "live";

/// A trust policy that lets API Gateway assume the role it is attached to. It carries none
/// of the characters that are significant in an `application/x-www-form-urlencoded` body.
const APIGATEWAY_TRUST: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","#,
    r#""Principal":{"Service":"apigateway.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#
);

/// The same policy for ANOTHER service's principal: a role this api cannot assume, which is
/// the only thing that differs between it and the row above.
const LAMBDA_TRUST: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","#,
    r#""Principal":{"Service":"lambda.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#
);

/// The object the reading rows fetch, and the bytes it is stored with.
const TEXT_KEY: &str = "report.txt";

/// Bytes that are not text at all, stored under a binary media type: what the stage serves
/// for them is the stored bytes, not a rendering of them.
const BINARY_KEY: &str = "pixel.png";

/// An object under a PREFIX, which a greedy path parameter carries whole.
const NESTED_KEY: &str = "notes/today.txt";

/// What one call answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: Bytes,
}

impl Answer {
    /// One response header's value.
    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }

    /// The body as text, for a JSON or XML answer.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    /// The body as JSON, `Null` when it is neither JSON nor present.
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

/// The three services this rule spans, over ONE engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_apigateway_backplanecontrolservice::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name`.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={AMBIENT_KEY}/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// Dispatch one request as it arrives on the wire.
async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Bytes,
) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let response = router
        .dispatch(builder.body(body).expect("a well-formed request"))
        .await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// One API Gateway control-plane call.
async fn apigw(router: &MultiServiceRouter, method: &str, path: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    dispatch(
        router,
        method,
        &format!("apigateway.{REGION}.amazonaws.com"),
        path,
        &[
            ("content-type", "application/json"),
            ("authorization", &authorization("apigateway")),
        ],
        Bytes::from(payload),
    )
    .await
}

/// One API Gateway control-plane call that has to succeed with `expected`.
async fn apigw_ok(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    expected: u16,
) -> Value {
    let answer = apigw(router, method, path, body).await;
    assert_eq!(
        answer.status,
        expected,
        "setup {case:?}: expected HTTP {expected} from {method} {path}, got {} with {:?}",
        answer.status,
        answer.text()
    );
    answer.json()
}

/// One Amazon S3 REST call, path-style as the emulator serves it.
async fn s3(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Bytes,
) -> Answer {
    let mut with_credential: Vec<(&str, &str)> = headers.to_vec();
    let credential = authorization("s3");
    with_credential.push(("authorization", &credential));
    dispatch(
        router,
        method,
        &format!("s3.{REGION}.amazonaws.com"),
        path,
        &with_credential,
        body,
    )
    .await
}

/// One IAM awsQuery call that has to succeed, answering its body.
async fn iam_ok(router: &MultiServiceRouter, case: &str, body: String) -> String {
    let answer = dispatch(
        router,
        "POST",
        "iam.amazonaws.com",
        "/",
        &[
            ("content-type", "application/x-www-form-urlencoded"),
            ("authorization", &authorization("iam")),
        ],
        Bytes::from(body),
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "setup {case:?}: expected HTTP 200 from IAM, got {} with {:?}",
        answer.status,
        answer.text()
    );
    answer.text()
}

/// One request to the api's own invoke endpoint — the host
/// `{rest-api-id}.execute-api.{region}.amazonaws.com`, which is the endpoint
/// `RestApi$disableExecuteApiEndpoint` documents — carrying no credential at all, as a
/// client of a public method sends it.
async fn invoke(
    router: &MultiServiceRouter,
    api: &str,
    verb: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Bytes,
) -> Answer {
    dispatch(
        router,
        verb,
        &format!("{api}.execute-api.{REGION}.amazonaws.com"),
        &format!("/{STAGE}{path}"),
        headers,
        body,
    )
    .await
}

/// The member `name` of a JSON object, as text.
fn member(case: &str, body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("setup {case:?}: no {name:?} in {body}"))
        .to_string()
}

/// One IAM role: created with `trust` as its trust policy and `document` as its one inline
/// policy, answering its ARN.
async fn role(
    router: &MultiServiceRouter,
    name: &str,
    trust: &str,
    document: &str,
) -> String {
    let created = iam_ok(
        router,
        &format!("CreateRole({name})"),
        format!(
            "Action=CreateRole&Version=2010-05-08&RoleName={name}\
             &AssumeRolePolicyDocument={trust}"
        ),
    )
    .await;
    let arn = xml_element(&created, "Arn")
        .unwrap_or_else(|| panic!("setup CreateRole({name}): no <Arn> in {created}"));
    iam_ok(
        router,
        &format!("PutRolePolicy({name})"),
        format!(
            "Action=PutRolePolicy&Version=2010-05-08&RoleName={name}\
             &PolicyName=stage-integration&PolicyDocument={document}"
        ),
    )
    .await;
    arn
}

/// The text of the first `<name>…</name>` element of an XML body.
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// One identity-based policy granting `actions` on `resource`, written with none of the
/// characters a form body treats specially.
fn policy(actions: &str, resource: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":[{actions}],"Resource":"{resource}"}}]}}"#
    )
}

/// One resource of the api under `parent`, answering its id.
async fn resource(router: &MultiServiceRouter, api: &str, parent: &str, part: &str) -> String {
    let created = apigw_ok(
        router,
        &format!("CreateResource({part})"),
        "POST",
        &format!("/restapis/{api}/resources/{parent}"),
        Some(json!({ "pathPart": part })),
        201,
    )
    .await;
    member(&format!("CreateResource({part})"), &created, "id")
}

/// One method of `resource` whose integration is the `AWS` service call `uri` names, under
/// `credentials` (none when it is `None`), with the integration responses that map what the
/// service answers: the default for a success, and one per class of refusal.
///
/// `mapped` names the integration request's path parameter the `uri`'s placeholder stands
/// for, when the `uri` carries one.
#[allow(clippy::too_many_arguments)]
async fn aws_method(
    router: &MultiServiceRouter,
    api: &str,
    resource_id: &str,
    verb: &str,
    uri: &str,
    credentials: Option<&str>,
    mapped: Option<(&str, &str)>,
    statuses: &[(&str, Option<&str>)],
) {
    let case = format!("{verb} {uri}");
    apigw_ok(
        router,
        &case,
        "PUT",
        &format!("/restapis/{api}/resources/{resource_id}/methods/{verb}"),
        Some(json!({ "authorizationType": "NONE" })),
        201,
    )
    .await;
    let mut integration = json!({
        "type": "AWS",
        // `integrationHttpMethod` travels under its `jsonName`, which is what
        // `smithy_query(query="operation",operation="PutIntegration")` reports for it.
        "httpMethod": verb,
        "uri": uri,
    });
    if let Some(credentials) = credentials {
        integration["credentials"] = json!(credentials);
    }
    if let Some((parameter, source)) = mapped {
        integration["requestParameters"] = json!({
            format!("integration.request.path.{parameter}"): source,
        });
    }
    apigw_ok(
        router,
        &case,
        "PUT",
        &format!("/restapis/{api}/resources/{resource_id}/methods/{verb}/integration"),
        Some(integration),
        201,
    )
    .await;
    for (status, pattern) in statuses {
        apigw_ok(
            router,
            &case,
            "PUT",
            &format!("/restapis/{api}/resources/{resource_id}/methods/{verb}/responses/{status}"),
            Some(json!({})),
            201,
        )
        .await;
        let mut response = json!({ "statusCode": status });
        if let Some(pattern) = pattern {
            response["selectionPattern"] = json!(pattern);
        }
        apigw_ok(
            router,
            &case,
            "PUT",
            &format!(
                "/restapis/{api}/resources/{resource_id}/methods/{verb}/integration/responses/{status}"
            ),
            Some(response),
            201,
        )
        .await;
    }
}

/// The integration responses every method here declares: the default (no pattern) for a
/// success, and one per refusal class, so what a client sees is the status the service
/// answered rather than one status for every outcome.
const MAPPED_STATUSES: &[(&str, Option<&str>)] = &[
    ("200", None),
    ("403", Some("403")),
    ("404", Some("404")),
    ("500", Some(r"5\d{2}")),
];

/// One row of the table: a request to the stage and what the records say it is answered
/// with.
struct Row {
    case: &'static str,
    verb: &'static str,
    path: String,
    /// The payload the request carries, for a write.
    body: Vec<u8>,
    content_type: Option<&'static str>,
    expected_status: u16,
    /// The bytes the method response carries, when the rule fixes them.
    expected_body: Option<Vec<u8>>,
    /// The content type the method response carries, when the rule fixes it.
    expected_content_type: Option<&'static str>,
}

#[tokio::test]
async fn a_deployed_method_integrating_an_aws_service_action_makes_that_call() {
    let router = emulator();

    // The objects the reading rows fetch, written through Amazon S3's own API so what the
    // stage serves is what that service holds.
    let text = b"the report, as stored".to_vec();
    let binary: Vec<u8> = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0xff, 0x00, 0xfe];
    let nested = b"today's note".to_vec();
    let created = s3(&router, "PUT", &format!("/{BUCKET}"), &[], Bytes::new()).await;
    assert_eq!(
        created.status,
        200,
        "setup: CreateBucket must succeed, got {} with {:?}",
        created.status,
        created.text()
    );
    for (key, bytes, media_type) in [
        (TEXT_KEY, &text, "text/plain"),
        (BINARY_KEY, &binary, "image/png"),
        (NESTED_KEY, &nested, "text/plain"),
    ] {
        let written = s3(
            &router,
            "PUT",
            &format!("/{BUCKET}/{key}"),
            &[("content-type", media_type)],
            Bytes::from(bytes.clone()),
        )
        .await;
        assert_eq!(
            written.status,
            200,
            "setup: PutObject({key}) must succeed, got {} with {:?}",
            written.status,
            written.text()
        );
    }

    // Three roles, each written by IAM's own `CreateRole`/`PutRolePolicy`: one this api can
    // assume that may act on this bucket's objects, one it can assume whose grant names
    // ANOTHER bucket, and one it cannot assume at all.
    let object_actions = r#""s3:GetObject","s3:PutObject","s3:DeleteObject""#;
    let allowed = role(
        &router,
        "stage-integration-allowed",
        APIGATEWAY_TRUST,
        &policy(object_actions, &format!("arn:aws:s3:::{BUCKET}/*")),
    )
    .await;
    let other_bucket = role(
        &router,
        "stage-integration-other-bucket",
        APIGATEWAY_TRUST,
        &policy(object_actions, "arn:aws:s3:::another-shelf/*"),
    )
    .await;
    let untrusted = role(
        &router,
        "stage-integration-untrusted",
        LAMBDA_TRUST,
        &policy(object_actions, &format!("arn:aws:s3:::{BUCKET}/*")),
    )
    .await;

    // The api: one resource per role and per uri form, each carrying the methods the rows
    // drive.
    let api_body = apigw_ok(
        &router,
        "CreateRestApi",
        "POST",
        "/restapis",
        Some(json!({
            "name": "stage-integration",
            // The stage passes the stored bytes of a binary object through, which is what
            // declaring its media type as binary says (`RestApi$binaryMediaTypes`).
            "binaryMediaTypes": ["image/png"],
        })),
        201,
    )
    .await;
    let api = member("CreateRestApi", &api_body, "id");
    let root = member("CreateRestApi", &api_body, "rootResourceId");

    let path_form = format!("arn:aws:apigateway:{REGION}:s3:path/{BUCKET}/{{object}}");
    let mapped = Some(("object", "method.request.path.key"));

    let objects = resource(&router, &api, &root, "object").await;
    let object_key = resource(&router, &api, &objects, "{key}").await;
    for verb in ["GET", "PUT", "HEAD", "DELETE"] {
        aws_method(
            &router,
            &api,
            &object_key,
            verb,
            &path_form,
            Some(&allowed),
            mapped,
            MAPPED_STATUSES,
        )
        .await;
    }

    // The ACTION form of the same call, which names the action itself rather than taking it
    // from the integration's verb.
    let acted = resource(&router, &api, &root, "acted").await;
    aws_method(
        &router,
        &api,
        &acted,
        "GET",
        &format!("arn:aws:apigateway:{REGION}:s3:action/GetObject&Bucket={BUCKET}&Key={TEXT_KEY}"),
        Some(&allowed),
        None,
        MAPPED_STATUSES,
    )
    .await;

    // A GREEDY path parameter, which carries a key under a prefix whole.
    let tree = resource(&router, &api, &root, "tree").await;
    let tree_item = resource(&router, &api, &tree, "{item+}").await;
    aws_method(
        &router,
        &api,
        &tree_item,
        "GET",
        &path_form,
        Some(&allowed),
        Some(("object", "method.request.path.item")),
        MAPPED_STATUSES,
    )
    .await;

    // The same read under the role whose grant names another bucket, under the role this
    // api cannot assume, and under no role at all.
    let denied = resource(&router, &api, &root, "denied").await;
    let denied_key = resource(&router, &api, &denied, "{key}").await;
    aws_method(
        &router,
        &api,
        &denied_key,
        "GET",
        &path_form,
        Some(&other_bucket),
        mapped,
        MAPPED_STATUSES,
    )
    .await;
    let unassumable = resource(&router, &api, &root, "unassumable").await;
    let unassumable_key = resource(&router, &api, &unassumable, "{key}").await;
    aws_method(
        &router,
        &api,
        &unassumable_key,
        "GET",
        &path_form,
        Some(&untrusted),
        mapped,
        MAPPED_STATUSES,
    )
    .await;
    let open = resource(&router, &api, &root, "open").await;
    let open_key = resource(&router, &api, &open, "{key}").await;
    aws_method(
        &router,
        &api,
        &open_key,
        "GET",
        &path_form,
        None,
        mapped,
        MAPPED_STATUSES,
    )
    .await;

    apigw_ok(
        &router,
        "CreateDeployment",
        "POST",
        &format!("/restapis/{api}/deployments"),
        Some(json!({ "stageName": STAGE })),
        201,
    )
    .await;

    let written_bytes = b"written through the stage".to_vec();
    let rows = vec![
        Row {
            case: "an object this role may read, through the path form",
            verb: "GET",
            path: format!("/object/{TEXT_KEY}"),
            body: Vec::new(),
            content_type: None,
            expected_status: 200,
            expected_body: Some(text.clone()),
            expected_content_type: Some("text/plain"),
        },
        Row {
            case: "bytes that are not text, under a binary media type",
            verb: "GET",
            path: format!("/object/{BINARY_KEY}"),
            body: Vec::new(),
            content_type: None,
            expected_status: 200,
            expected_body: Some(binary.clone()),
            expected_content_type: Some("image/png"),
        },
        Row {
            case: "a key under a prefix, carried by a greedy path parameter",
            verb: "GET",
            path: format!("/tree/{NESTED_KEY}"),
            body: Vec::new(),
            content_type: None,
            expected_status: 200,
            expected_body: Some(nested.clone()),
            expected_content_type: Some("text/plain"),
        },
        Row {
            case: "the action form, which names the action itself",
            verb: "GET",
            path: "/acted".to_string(),
            body: Vec::new(),
            content_type: None,
            expected_status: 200,
            expected_body: Some(text.clone()),
            expected_content_type: Some("text/plain"),
        },
        Row {
            case: "a key the bucket holds nothing under",
            verb: "GET",
            path: "/object/absent.txt".to_string(),
            body: Vec::new(),
            content_type: None,
            // Amazon S3 answers `NoSuchKey` with 404, and the integration response whose
            // pattern matches that status is what the client sees.
            expected_status: 404,
            expected_body: None,
            expected_content_type: None,
        },
        Row {
            case: "a write this role may make",
            verb: "PUT",
            path: "/object/written.txt".to_string(),
            body: written_bytes.clone(),
            content_type: Some("text/plain"),
            expected_status: 200,
            expected_body: None,
            expected_content_type: None,
        },
        Row {
            case: "a head of a stored object",
            verb: "HEAD",
            path: format!("/object/{TEXT_KEY}"),
            body: Vec::new(),
            content_type: None,
            expected_status: 200,
            expected_body: Some(Vec::new()),
            expected_content_type: None,
        },
        Row {
            case: "a role whose grant names another bucket",
            verb: "GET",
            path: format!("/denied/{TEXT_KEY}"),
            body: Vec::new(),
            content_type: None,
            expected_status: 403,
            expected_body: None,
            expected_content_type: None,
        },
        Row {
            case: "a role this api cannot assume",
            verb: "GET",
            path: format!("/unassumable/{TEXT_KEY}"),
            body: Vec::new(),
            content_type: None,
            // No call can be made under a role nothing can assume, so the integration could
            // not be performed at all: API Gateway's own `API_CONFIGURATION_ERROR`.
            expected_status: 500,
            expected_body: None,
            expected_content_type: None,
        },
        Row {
            case: "an integration naming no role at all",
            verb: "GET",
            path: format!("/open/{TEXT_KEY}"),
            body: Vec::new(),
            content_type: None,
            expected_status: 200,
            expected_body: Some(text.clone()),
            expected_content_type: Some("text/plain"),
        },
    ];

    for row in &rows {
        let headers: Vec<(&str, &str)> = row
            .content_type
            .map(|media_type| vec![("content-type", media_type)])
            .unwrap_or_default();
        let answer = invoke(
            &router,
            &api,
            row.verb,
            &row.path,
            &headers,
            Bytes::from(row.body.clone()),
        )
        .await;
        assert_eq!(
            answer.status,
            row.expected_status,
            "case {:?}: expected HTTP {}, got {} with {:?}",
            row.case,
            row.expected_status,
            answer.status,
            answer.text()
        );
        if let Some(expected) = &row.expected_body {
            assert_eq!(
                answer.body.as_ref(),
                expected.as_slice(),
                "case {:?}: expected the stored bytes, got {:?}",
                row.case,
                answer.text()
            );
        }
        if let Some(expected) = row.expected_content_type {
            assert_eq!(
                answer.header("content-type").as_deref(),
                Some(expected),
                "case {:?}: expected the content type the object is stored under, got {:?}",
                row.case,
                answer.header("content-type")
            );
        }
        // Whatever the outcome, it is API Gateway's own answer and never the front door's
        // refusal for a host no service claims.
        assert!(
            !answer.text().contains("UnknownService"),
            "case {:?}: the invoke endpoint must be served by the stage, got {:?}",
            row.case,
            answer.text()
        );
    }

    // The 500 is API Gateway's own integration-failure response, body included, and it
    // carries nothing of the object the call would have read.
    let unassumable_answer = invoke(
        &router,
        &api,
        "GET",
        &format!("/unassumable/{TEXT_KEY}"),
        &[],
        Bytes::new(),
    )
    .await;
    assert_eq!(
        unassumable_answer.json(),
        json!({ "message": "Internal server error" }),
        "a role this api cannot assume must be answered by the api's own \
         API_CONFIGURATION_ERROR response, got {:?}",
        unassumable_answer.text()
    );

    // The refusal of the role whose grant names another bucket is the SERVICE's own, so it
    // names the code that service refuses an unauthorized call with.
    let denied_answer = invoke(
        &router,
        &api,
        "GET",
        &format!("/denied/{TEXT_KEY}"),
        &[],
        Bytes::new(),
    )
    .await;
    assert!(
        denied_answer.text().contains("AccessDenied"),
        "a call the role may not make must carry the service's own refusal, got {:?}",
        denied_answer.text()
    );
    assert!(
        !denied_answer.text().contains("the report, as stored"),
        "a refused call must carry nothing of the object, got {:?}",
        denied_answer.text()
    );

    // ROUND TRIP: the write the stage performed is in Amazon S3, read back through that
    // service's own API — the integration wrote an object rather than reporting that it had.
    let read_back = s3(
        &router,
        "GET",
        &format!("/{BUCKET}/written.txt"),
        &[],
        Bytes::new(),
    )
    .await;
    assert_eq!(
        read_back.status,
        200,
        "the object the stage wrote must be readable through Amazon S3, got {} with {:?}",
        read_back.status,
        read_back.text()
    );
    assert_eq!(
        read_back.body.as_ref(),
        written_bytes.as_slice(),
        "Amazon S3 must hold the bytes the stage wrote, got {:?}",
        read_back.text()
    );

    // And the delete the stage performs removes it: the answer maps to the default
    // integration response (Amazon S3 answers a delete 204, which no pattern matches), and
    // the object is gone from the service that held it.
    let deleted = invoke(
        &router,
        &api,
        "DELETE",
        "/object/written.txt",
        &[],
        Bytes::new(),
    )
    .await;
    assert_eq!(
        deleted.status,
        200,
        "the delete must map to the integration response with no pattern, got {} with {:?}",
        deleted.status,
        deleted.text()
    );
    let gone = s3(
        &router,
        "GET",
        &format!("/{BUCKET}/written.txt"),
        &[],
        Bytes::new(),
    )
    .await;
    assert_eq!(
        gone.status,
        404,
        "the object the stage deleted must be gone from Amazon S3, got {} with {:?}",
        gone.status,
        gone.text()
    );
}

/// One row of the bucket table: a request to the stage and what the records say it is
/// answered with.
struct BucketRow {
    case: &'static str,
    verb: &'static str,
    path: &'static str,
    expected_status: u16,
    /// The text every one of these has to appear in the method response's body, which for a
    /// listing is Amazon S3's own `ListBucketResult` document.
    expected_contains: &'static [&'static str],
    /// Text the method response must NOT carry.
    expected_absent: &'static [&'static str],
}

/// The bucket this test's own rows address, and a second one the account does not hold.
const LISTED_BUCKET: &str = "stage-integration-listed";
const ABSENT_BUCKET: &str = "stage-integration-absent";

/// RULE (the same one, over the other resource an Amazon S3 `path` form can name): the
/// resource of an `AWS` integration's `uri` IS Amazon S3's own REST path, so a `path` naming
/// a bucket and nothing under it addresses the BUCKET — `GET` is the bucket's own listing
/// (`ListObjects`, which Amazon S3 binds to `GET /{Bucket}` and answers as the
/// `ListBucketResult` document) and `HEAD` is `HeadBucket` — authorized by `s3:ListBucket`
/// on the bucket's ARN, and the method response carries the service's own answer.
///
/// The rows vary what the rule generalises over: the FORM the `uri` names the call in (the
/// path form and the action form), the ACTION (the listing and the head), the role (one that
/// may list this bucket, one whose grant names another), and the bucket (one the account
/// holds and one it does not).
#[tokio::test]
async fn a_deployed_method_integrating_an_aws_service_action_can_address_a_bucket() {
    let router = emulator();

    // The bucket and the two objects its listing reports, written through Amazon S3's own
    // API so what the stage serves is what that service holds.
    let created = s3(&router, "PUT", &format!("/{LISTED_BUCKET}"), &[], Bytes::new()).await;
    assert_eq!(
        created.status,
        200,
        "setup: CreateBucket must succeed, got {} with {:?}",
        created.status,
        created.text()
    );
    for key in [TEXT_KEY, NESTED_KEY] {
        let written = s3(
            &router,
            "PUT",
            &format!("/{LISTED_BUCKET}/{key}"),
            &[("content-type", "text/plain")],
            Bytes::from("listed".to_string()),
        )
        .await;
        assert_eq!(
            written.status,
            200,
            "setup: PutObject({key}) must succeed, got {} with {:?}",
            written.status,
            written.text()
        );
    }

    // Two roles this api can assume: one granted the bucket action on THIS bucket, one on
    // another bucket's ARN.
    let list_action = r#""s3:ListBucket""#;
    let allowed = role(
        &router,
        "stage-listing-allowed",
        APIGATEWAY_TRUST,
        &policy(list_action, &format!("arn:aws:s3:::{LISTED_BUCKET}")),
    )
    .await;
    let other_bucket = role(
        &router,
        "stage-listing-other-bucket",
        APIGATEWAY_TRUST,
        &policy(list_action, "arn:aws:s3:::another-shelf"),
    )
    .await;

    let api_body = apigw_ok(
        &router,
        "CreateRestApi",
        "POST",
        "/restapis",
        Some(json!({ "name": "stage-listing" })),
        201,
    )
    .await;
    let api = member("CreateRestApi", &api_body, "id");
    let root = member("CreateRestApi", &api_body, "rootResourceId");

    // The PATH form naming the bucket alone, with the listing and the head on it.
    let bucket_path = format!("arn:aws:apigateway:{REGION}:s3:path/{LISTED_BUCKET}");
    let listing = resource(&router, &api, &root, "listing").await;
    for verb in ["GET", "HEAD"] {
        aws_method(
            &router,
            &api,
            &listing,
            verb,
            &bucket_path,
            Some(&allowed),
            None,
            MAPPED_STATUSES,
        )
        .await;
    }

    // The ACTION form of the same listing.
    let listed = resource(&router, &api, &root, "listed").await;
    aws_method(
        &router,
        &api,
        &listed,
        "GET",
        &format!("arn:aws:apigateway:{REGION}:s3:action/ListObjects&Bucket={LISTED_BUCKET}"),
        Some(&allowed),
        None,
        MAPPED_STATUSES,
    )
    .await;

    // The same listing under the role whose grant names another bucket.
    let denied = resource(&router, &api, &root, "listing-denied").await;
    aws_method(
        &router,
        &api,
        &denied,
        "GET",
        &bucket_path,
        Some(&other_bucket),
        None,
        MAPPED_STATUSES,
    )
    .await;

    // A listing of a bucket the account does not hold, under a role granted every bucket.
    let every_bucket = role(
        &router,
        "stage-listing-every-bucket",
        APIGATEWAY_TRUST,
        &policy(list_action, "arn:aws:s3:::*"),
    )
    .await;
    let absent = resource(&router, &api, &root, "listing-absent").await;
    aws_method(
        &router,
        &api,
        &absent,
        "GET",
        &format!("arn:aws:apigateway:{REGION}:s3:path/{ABSENT_BUCKET}"),
        Some(&every_bucket),
        None,
        MAPPED_STATUSES,
    )
    .await;

    apigw_ok(
        &router,
        "CreateDeployment",
        "POST",
        &format!("/restapis/{api}/deployments"),
        Some(json!({ "stageName": STAGE })),
        201,
    )
    .await;

    const LISTING_ELEMENTS: &[&str] = &[
        "<ListBucketResult",
        "<Name>stage-integration-listed</Name>",
        "<Key>report.txt</Key>",
        "<Key>notes/today.txt</Key>",
    ];
    let rows: &[BucketRow] = &[
        BucketRow {
            case: "the bucket's own listing, through the path form naming the bucket",
            verb: "GET",
            path: "/listing",
            expected_status: 200,
            expected_contains: LISTING_ELEMENTS,
            expected_absent: &["UnknownService"],
        },
        BucketRow {
            case: "the same listing through the action form",
            verb: "GET",
            path: "/listed",
            expected_status: 200,
            expected_contains: LISTING_ELEMENTS,
            expected_absent: &["UnknownService"],
        },
        BucketRow {
            case: "a head of the bucket, which carries no body",
            verb: "HEAD",
            path: "/listing",
            expected_status: 200,
            expected_contains: &[],
            expected_absent: &["<ListBucketResult", "UnknownService"],
        },
        BucketRow {
            case: "a role whose grant names another bucket",
            verb: "GET",
            path: "/listing-denied",
            expected_status: 403,
            // The refusal is the SERVICE's own, in its own error envelope.
            expected_contains: &["AccessDenied"],
            expected_absent: &["<ListBucketResult", "UnknownService"],
        },
        BucketRow {
            case: "a bucket the account does not hold",
            verb: "GET",
            path: "/listing-absent",
            // Amazon S3 answers `NoSuchBucket` with 404, and the integration response whose
            // pattern matches that status is what the client sees.
            expected_status: 404,
            expected_contains: &["NoSuchBucket"],
            expected_absent: &["<ListBucketResult", "UnknownService"],
        },
    ];

    for row in rows {
        let answer = invoke(&router, &api, row.verb, row.path, &[], Bytes::new()).await;
        assert_eq!(
            answer.status,
            row.expected_status,
            "case {:?}: expected HTTP {}, got {} with {:?}",
            row.case,
            row.expected_status,
            answer.status,
            answer.text()
        );
        let body = answer.text();
        for expected in row.expected_contains {
            assert!(
                body.contains(expected),
                "case {:?}: expected the body to carry {expected:?}, got {body:?}",
                row.case
            );
        }
        for absent in row.expected_absent {
            assert!(
                !body.contains(absent),
                "case {:?}: expected the body to carry nothing of {absent:?}, got {body:?}",
                row.case
            );
        }
    }

    // ROUND TRIP: the listing the stage served is the bucket's own, read back through Amazon
    // S3's own API — the two documents report the same keys.
    let read_back = s3(
        &router,
        "GET",
        &format!("/{LISTED_BUCKET}"),
        &[],
        Bytes::new(),
    )
    .await;
    assert_eq!(
        read_back.status,
        200,
        "the bucket must be listable through Amazon S3, got {} with {:?}",
        read_back.status,
        read_back.text()
    );
    let through_the_stage = invoke(&router, &api, "GET", "/listing", &[], Bytes::new()).await;
    for key in [TEXT_KEY, NESTED_KEY] {
        let element = format!("<Key>{key}</Key>");
        assert!(
            read_back.text().contains(&element) && through_the_stage.text().contains(&element),
            "the stage's listing and Amazon S3's own must report {key:?} alike, got {:?} and \
             {:?}",
            through_the_stage.text(),
            read_back.text()
        );
    }
}
