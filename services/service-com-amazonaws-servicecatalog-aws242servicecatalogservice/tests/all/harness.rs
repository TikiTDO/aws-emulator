// SPDX-License-Identifier: CC0-1.0

//! The awsJson1_1 wire harness every suite in this crate drives the service through.
//!
//! One emulator over one shared engine, one `POST /` request per call with the
//! `X-Amz-Target` and SigV4 credential scope a real Service Catalog client sends, and the
//! small readers a suite needs to assert on a response body. It lives in its own module
//! so a suite does not carry a second copy of the transport.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix: the service shape's name,
/// `com.amazonaws.servicecatalog#AWS242ServiceCatalogService`.
pub(crate) const TARGET_PREFIX: &str = "AWS242ServiceCatalogService";

/// The region the harness signs for unless a case picks another.
pub(crate) const DEFAULT_REGION: &str = "us-east-1";

/// The canonical account id every emulated ARN carries
/// (`engine::EMULATOR_ACCOUNT_ID`).
pub(crate) const ACCOUNT_ID: &str = "000000000000";

/// One emulator: a single shared engine behind the router, exactly as
/// `core/router` assembles it.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The harness signs nothing, so authentication is off for this router only.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_servicecatalog_aws242servicecatalogservice::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// The bucket a suite stages its templates in, as an administrator stages a product's
/// template before creating the product from it.
pub(crate) const TEMPLATE_BUCKET: &str = "service-catalog-staged-templates";

/// One emulator carrying Service Catalog, the service that owns a CloudFormation template,
/// and the object store a staged template lives in — all three on the ONE shared engine,
/// exactly as `core/router/src/lib.rs` mounts them.
///
/// A version's template is not held by Service Catalog: the version names it, and reading
/// it is a call into CloudFormation, which reads the staged object out of Amazon S3. So a
/// suite that asserts what a version declares — or what a plan over that version would
/// change — must have those services there, the same way the CloudFormation suites mount
/// the services a stack's resources are created in.
pub(crate) fn emulator_with_template_services() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The harness signs nothing, so authentication is off for this router only.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_servicecatalog_aws242servicecatalogservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router
}

/// Drive one Amazon S3 request through the same router, as a client's own SDK sends it.
pub(crate) async fn s3(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: &[u8],
) -> (u16, String) {
    let request = ::http::Request::builder()
        .method(method)
        .uri(format!("https://s3.{DEFAULT_REGION}.amazonaws.com{path}"))
        .header("host", format!("s3.{DEFAULT_REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTHARNESS/20260101/{DEFAULT_REGION}/s3/\
                 aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .body(Bytes::copy_from_slice(body))
        .expect("a well-formed S3 request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let read = String::from_utf8_lossy(&response.into_body()).to_string();
    (status, read)
}

/// Create the staging bucket, once per router.
pub(crate) async fn create_template_bucket(router: &MultiServiceRouter) {
    let (status, body) = s3(router, "PUT", &format!("/{TEMPLATE_BUCKET}"), b"").await;
    assert_eq!(
        status, 200,
        "the staging bucket must be created first, got {status} with {body}"
    );
}

/// Stage one template object, as an administrator's own `PutObject` stores it.
pub(crate) async fn stage_template(router: &MultiServiceRouter, key: &str, template: &str) {
    let (status, body) = s3(
        router,
        "PUT",
        &format!("/{TEMPLATE_BUCKET}/{key}"),
        template.as_bytes(),
    )
    .await;
    assert_eq!(
        status, 200,
        "the template {key} must be staged first, got {status} with {body}"
    );
}

/// The path-style URL of one staged template object, which is what
/// `ProvisioningArtifactProperties$Info` takes as "the URL of the CloudFormation
/// template in Amazon S3".
pub(crate) fn staged_template_url(key: &str) -> String {
    format!("https://s3.{DEFAULT_REGION}.amazonaws.com/{TEMPLATE_BUCKET}/{key}")
}

/// Send one awsJson1_1 request signed for `region` and return its status and body.
pub(crate) async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTHARNESS/20260101/{region}/servicecatalog/\
                 aws4_request, SignedHeaders=host;x-amz-target, Signature=0"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "{operation}: response body is not JSON ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// Send one request in [`DEFAULT_REGION`].
pub(crate) async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    call_in(router, DEFAULT_REGION, operation, body).await
}

/// The wire error code of a failure body: the last segment of awsJson's `__type`.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .map(|t| t.split('#').next_back().unwrap_or(t).to_string())
        .unwrap_or_else(|| format!("<no __type in {body}>"))
}

/// The error message of a failure body, whichever case the wire member carries.
///
/// The one reader every suite in this crate asserts a refusal's wording through, so a
/// suite does not carry a second copy of it.
pub(crate) fn message(body: &Value) -> String {
    body["message"]
        .as_str()
        .or(body["Message"].as_str())
        .unwrap_or_else(|| panic!("no message in {body}"))
        .to_string()
}

/// A `Tags` list from a response, as a key -> value map.
pub(crate) fn tag_map(case: &str, tags: &Value) -> BTreeMap<String, String> {
    let list = tags
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a Tags list, got {tags}"));
    list.iter()
        .map(|tag| {
            let key = tag["Key"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a tag has no Key: {tag}"));
            let value = tag["Value"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a tag has no Value: {tag}"));
            (key.to_string(), value.to_string())
        })
        .collect()
}

/// `pairs` as the modelled `Tags`/`AddTags` list.
pub(crate) fn tag_list(pairs: &[(&str, &str)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({"Key": key, "Value": value}))
            .collect(),
    )
}

/// Create one portfolio and answer the id the service minted.
///
/// The one portfolio seeder every suite in this crate drives, so two suites cannot seed
/// a portfolio in two different ways.
pub(crate) async fn create_portfolio(
    router: &MultiServiceRouter,
    display_name: &str,
    provider: &str,
    tags: &[(&str, &str)],
    token: &str,
) -> String {
    let mut request = json!({
        "DisplayName": display_name,
        "ProviderName": provider,
        "IdempotencyToken": token,
    });
    if !tags.is_empty() {
        request["Tags"] = tag_list(tags);
    }
    let (status, created) = call(router, "CreatePortfolio", &request).await;
    assert_eq!(
        status, 200,
        "CreatePortfolio {display_name:?}: expected 200, got {status} with {created}"
    );
    created["PortfolioDetail"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreatePortfolio {display_name:?}: no Id in {created}"))
        .to_string()
}

/// Create one CloudFormation product and answer the id the service minted.
pub(crate) async fn create_product(
    router: &MultiServiceRouter,
    name: &str,
    token: &str,
) -> String {
    create_product_of_type(router, name, "CLOUD_FORMATION_TEMPLATE", token).await
}

/// Create one product of `product_type` and answer the id the service minted.
///
/// The accepted values are the model's own
/// (`smithy_query query="shape" shape="ProductType"`: CLOUD_FORMATION_TEMPLATE,
/// MARKETPLACE, TERRAFORM_OPEN_SOURCE, TERRAFORM_CLOUD and EXTERNAL), and the first
/// version's `Type` is the `ProvisioningArtifactType` of that product type
/// (`smithy_query query="shape" shape="ProvisioningArtifactType"`:
/// CLOUD_FORMATION_TEMPLATE, MARKETPLACE_AMI, MARKETPLACE_CAR, TERRAFORM_OPEN_SOURCE,
/// TERRAFORM_CLOUD and EXTERNAL — the two enums share every name but MARKETPLACE's).
pub(crate) async fn create_product_of_type(
    router: &MultiServiceRouter,
    name: &str,
    product_type: &str,
    token: &str,
) -> String {
    create_product_with_info(
        router,
        name,
        product_type,
        // The accepted `Info` keys are the model's own:
        // `ProvisioningArtifactProperties$Info` documents
        // "Keys accepted: [ LoadTemplateFromURL, ImportFromPhysicalId ]".
        &json!({"LoadTemplateFromURL": "https://example.invalid/template.json"}),
        token,
    )
    .await
}

/// Create one product of `product_type` whose first version names `info` as its template
/// source, and answer the id the service minted.
///
/// The one `CreateProduct` caller of this crate's suites, so a suite that needs a version
/// pointing at a template it staged does not spell a second create of its own.
pub(crate) async fn create_product_with_info(
    router: &MultiServiceRouter,
    name: &str,
    product_type: &str,
    info: &Value,
    token: &str,
) -> String {
    let artifact_type = match product_type {
        "MARKETPLACE" => "MARKETPLACE_AMI",
        shared => shared,
    };
    let (status, created) = call(
        router,
        "CreateProduct",
        &json!({
            "Name": name,
            "Owner": "acme",
            "ProductType": product_type,
            "ProvisioningArtifactParameters": {
                "Name": "v1",
                "Type": artifact_type,
                "Info": info,
            },
            "IdempotencyToken": token,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateProduct {name:?}: expected 200, got {status} with {created}"
    );
    created["ProductViewDetail"]["ProductViewSummary"]["ProductId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateProduct {name:?}: no ProductId in {created}"))
        .to_string()
}

/// The id of the one provisioning artifact `create_product_of_type` minted for
/// `product_id` — the product version a provisioning request names.
///
/// Read through `ListProvisioningArtifacts`, the ADMIN view of a product's versions: a
/// product a case has not yet put in a portfolio is outside the end-user view, which
/// reaches a product only through a launch path.
pub(crate) async fn first_provisioning_artifact_id(
    router: &MultiServiceRouter,
    product_id: &str,
) -> String {
    let (status, listed) = call(
        router,
        "ListProvisioningArtifacts",
        &json!({"ProductId": product_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "ListProvisioningArtifacts {product_id}: expected 200, got {status} with {listed}"
    );
    listed["ProvisioningArtifactDetails"][0]["Id"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("ListProvisioningArtifacts {product_id}: no provisioning artifact in {listed}")
        })
        .to_string()
}

/// Associate a product with a portfolio: the one stored relationship every suite that
/// needs one creates the same way.
pub(crate) async fn associate_product_with_portfolio(
    router: &MultiServiceRouter,
    product_id: &str,
    portfolio_id: &str,
) {
    let (status, body) = call(
        router,
        "AssociateProductWithPortfolio",
        &json!({"ProductId": product_id, "PortfolioId": portfolio_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "AssociateProductWithPortfolio({product_id}, {portfolio_id}): expected 200, got {status} \
         with {body}"
    );
}

/// A `Parameters` document of the shape each constraint type documents
/// (`smithy_query query="doc" shape="CreateConstraintInput" member="Parameters"`): the
/// emulator stores it verbatim, so the only thing that matters here is that each caller
/// sends the document its type calls for.
pub(crate) fn constraint_parameters(constraint_type: &str) -> String {
    match constraint_type {
        "LAUNCH" => r#"{"RoleArn":"arn:aws:iam::000000000000:role/SCBasicLaunchRole"}"#.to_string(),
        "NOTIFICATION" => {
            r#"{"NotificationArns":["arn:aws:sns:us-east-1:000000000000:topic"]}"#.to_string()
        }
        "STACKSET" => r#"{"AccountList":["000000000000"],"RegionList":["us-east-1"],"AdminRole":"arn:aws:iam::000000000000:role/AWSCloudFormationStackSetAdministrationRole","ExecutionRole":"AWSCloudFormationStackSetExecutionRole"}"#
            .to_string(),
        _ => r#"{"Rules":{"Required":{"Assertions":[]}}}"#.to_string(),
    }
}

/// Create one constraint on a portfolio-product pair.
pub(crate) async fn create_constraint(
    router: &MultiServiceRouter,
    portfolio_id: &str,
    product_id: &str,
    constraint_type: &str,
    description: &str,
    token: &str,
) {
    let (status, created) = call(
        router,
        "CreateConstraint",
        &json!({
            "PortfolioId": portfolio_id,
            "ProductId": product_id,
            "Type": constraint_type,
            "Description": description,
            "Parameters": constraint_parameters(constraint_type),
            "IdempotencyToken": token,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateConstraint {constraint_type} on ({portfolio_id}, {product_id}): expected 200, got \
         {status} with {created}"
    );
}

/// AWS mints a Service Catalog id as a `<prefix>-` followed by 12 lowercase base-36
/// characters, which the modelled `Id` pattern `^[a-zA-Z0-9_\-]*$` admits.
pub(crate) fn is_catalog_id(prefix: &str, id: &str) -> bool {
    id.strip_prefix(&format!("{prefix}-")).is_some_and(|rest| {
        rest.len() == 12
            && rest
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    })
}
