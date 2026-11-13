// SPDX-License-Identifier: CC0-1.0

//! The modelled CLIENT errors of the catalog resources — constraints, products and
//! their versions, self-service actions, TagOptions and portfolio principals — over
//! the wire, driven as awsJson1_1 requests through the same `MultiServiceRouter` the
//! production emulator mounts.
//!
//! The rule under test: an operation that addresses a catalog resource answers the
//! modelled client error (HTTP 400) its model declares for the request it cannot
//! serve — `ResourceNotFoundException` for an id or name no stored resource has,
//! `DuplicateResourceException` for a create of a resource that already exists,
//! `InvalidParametersException` for a parameter combination it cannot act on — and
//! never a server fault, and never a hollow 200.
//!
//! Every success assertion here is about STORED state: a response is checked against
//! what an earlier call persisted, never against a value this file hands the service
//! and immediately reads back out of the same response.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix: the service shape's name,
/// `com.amazonaws.servicecatalog#AWS242ServiceCatalogService` (`OPERATIONS.md`,
/// "Request routing").
const TARGET_PREFIX: &str = "AWS242ServiceCatalogService";

/// One emulator: a single shared engine behind the router, exactly as
/// `core/router` assembles it.
fn emulator() -> MultiServiceRouter {
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

/// Send one awsJson1_1 request and return its status and body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDTESTHARNESS/20260101/us-east-1/servicecatalog/\
             aws4_request, SignedHeaders=host;x-amz-target, Signature=0",
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

/// The wire error code of a failure body: the last segment of awsJson's `__type`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .map(|t| t.split('#').next_back().unwrap_or(t).to_string())
        .unwrap_or_else(|| format!("<no __type in {body}>"))
}

/// A string member of a response, or a panic naming the case.
fn field<'a>(case: &str, body: &'a Value, pointer: &str) -> &'a str {
    body.pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a string at {pointer} in {body}"))
}

/// A created resource of every kind the catalog addresses by id, so a not-found
/// assertion is about the id and not about an empty store.
struct Seeded {
    portfolio_id: String,
    product_id: String,
    product_name: String,
    provisioning_artifact_id: String,
    constraint_id: String,
    service_action_id: String,
    tag_option_id: String,
}

/// The template source a CloudFormation product is created from. The accepted
/// `Info` keys are the model's own: `ProvisioningArtifactProperties$Info` documents
/// "Keys accepted: [ LoadTemplateFromURL, ImportFromPhysicalId ]".
fn artifact_parameters(name: &str) -> Value {
    json!({
        "Name": name,
        "Description": "the first version",
        "Info": {"LoadTemplateFromURL": "https://example.invalid/template.json"},
        "Type": "CLOUD_FORMATION_TEMPLATE",
    })
}

/// Create one of each catalog resource, returning the ids the service minted.
async fn seed(router: &MultiServiceRouter) -> Seeded {
    let (status, portfolio) = call(
        router,
        "CreatePortfolio",
        &json!({
            "DisplayName": "seed-portfolio",
            "ProviderName": "acme",
            "IdempotencyToken": "seed-portfolio",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "seed CreatePortfolio: expected 200, got {status} with {portfolio}"
    );
    let portfolio_id = field("seed", &portfolio, "/PortfolioDetail/Id").to_string();

    let product_name = "seed-product";
    let (status, product) = call(
        router,
        "CreateProduct",
        &json!({
            "Name": product_name,
            "Owner": "acme",
            "ProductType": "CLOUD_FORMATION_TEMPLATE",
            "ProvisioningArtifactParameters": artifact_parameters("v1"),
            "IdempotencyToken": "seed-product",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "seed CreateProduct: expected 200, got {status} with {product}"
    );
    let product_id = field("seed", &product, "/ProductViewDetail/ProductViewSummary/ProductId")
        .to_string();
    let provisioning_artifact_id =
        field("seed", &product, "/ProvisioningArtifactDetail/Id").to_string();

    let (status, constraint) = call(
        router,
        "CreateConstraint",
        &json!({
            "PortfolioId": portfolio_id,
            "ProductId": product_id,
            "Type": "NOTIFICATION",
            "Parameters": r#"{"NotificationArns":["arn:aws:sns:us-east-1:000000000000:topic"]}"#,
            "IdempotencyToken": "seed-constraint",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "seed CreateConstraint: expected 200, got {status} with {constraint}"
    );
    let constraint_id = field("seed", &constraint, "/ConstraintDetail/ConstraintId").to_string();

    let (status, action) = call(
        router,
        "CreateServiceAction",
        &json!({
            "Name": "seed-action",
            "DefinitionType": "SSM_AUTOMATION",
            // The definition keys are the modelled `ServiceActionDefinitionKey`
            // values (Name | Version | AssumeRole | Parameters).
            "Definition": {"Name": "AWS-RestartEC2Instance", "Version": "1"},
            "IdempotencyToken": "seed-action",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "seed CreateServiceAction: expected 200, got {status} with {action}"
    );
    let service_action_id =
        field("seed", &action, "/ServiceActionDetail/ServiceActionSummary/Id").to_string();

    let (status, option) = call(
        router,
        "CreateTagOption",
        &json!({"Key": "seed-key", "Value": "seed-value"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "seed CreateTagOption: expected 200, got {status} with {option}"
    );
    let tag_option_id = field("seed", &option, "/TagOptionDetail/Id").to_string();

    Seeded {
        portfolio_id,
        product_id,
        product_name: product_name.to_string(),
        provisioning_artifact_id,
        constraint_id,
        service_action_id,
        tag_option_id,
    }
}

/// Every operation that addresses a catalog resource by id refuses an id no
/// resource has with the modelled 400 `ResourceNotFoundException`, whatever the
/// id's shape — never a 5xx, never a 501, never a hollow 200.
#[tokio::test]
async fn an_unknown_resource_id_is_resource_not_found() {
    let router = emulator();
    let seeded = seed(&router).await;

    // Rows differ along the shape of the absent identifier: a well-formed id of the
    // right kind (the `{}` stands for the kind's own prefix), one of ANOTHER kind, a
    // value no id form admits, and the shortest value the modelled `Id` length
    // 1..100 allows.
    let shapes: [(&str, &str); 4] = [
        ("well-formed but unused", "{}-000000000000"),
        ("id of another resource kind", "port-zzzzzzzzzzzz"),
        ("not an id of any kind", "lorem"),
        ("at the Id length bound", "x"),
    ];

    // One row per operation that looks a resource up, with the member it looks it up
    // through and the id prefix of the kind it addresses.
    let lookups: [(&str, &str, &str); 6] = [
        ("DescribeConstraint", "Id", "cons"),
        ("DescribeProduct", "Id", "prod"),
        ("ListProvisioningArtifacts", "ProductId", "prod"),
        ("DescribeServiceAction", "Id", "act"),
        ("DescribeTagOption", "Id", "tag"),
        ("ListPrincipalsForPortfolio", "PortfolioId", "port"),
    ];

    for (operation, member, prefix) in lookups {
        for (case, shape) in shapes {
            let unknown = shape.replace("{}", prefix);
            let (status, refused) = call(&router, operation, &json!({member: unknown})).await;
            assert_eq!(
                status, 400,
                "case {case:?}/{operation}: expected 400 for {member} {unknown:?}, got {status} \
                 with {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "ResourceNotFoundException",
                "case {case:?}/{operation}: unexpected error for {member} {unknown:?} in \
                 {refused}"
            );
        }
    }

    // A product named by a `Name` no product carries is the same refusal through the
    // other identifying member.
    let (status, refused) = call(&router, "DescribeProduct", &json!({"Name": "no-such-name"})).await;
    assert_eq!(
        status, 400,
        "case \"unknown Name\": expected 400, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ResourceNotFoundException",
        "case \"unknown Name\": unexpected error in {refused}"
    );

    // Naming no identifier at all is a bad parameter, not a missing resource.
    let (status, refused) = call(&router, "DescribeProduct", &json!({})).await;
    assert_eq!(
        status, 400,
        "case \"neither Id nor Name\": expected 400, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "InvalidParametersException",
        "case \"neither Id nor Name\": unexpected error in {refused}"
    );

    // And none of those refusals disturbed the stored resources: each is still
    // readable through its own describe.
    for (case, operation, body) in [
        (
            "constraint",
            "DescribeConstraint",
            json!({"Id": seeded.constraint_id}),
        ),
        (
            "product",
            // The ADMIN read of a product: the seeded product sits in no portfolio, so
            // it is outside the catalog the end-user `DescribeProduct` reaches.
            "DescribeProductAsAdmin",
            json!({"Id": seeded.product_id}),
        ),
        (
            "versions",
            "ListProvisioningArtifacts",
            json!({"ProductId": seeded.product_id}),
        ),
        (
            "service action",
            "DescribeServiceAction",
            json!({"Id": seeded.service_action_id}),
        ),
        (
            "tag option",
            "DescribeTagOption",
            json!({"Id": seeded.tag_option_id}),
        ),
        (
            "principals",
            "ListPrincipalsForPortfolio",
            json!({"PortfolioId": seeded.portfolio_id}),
        ),
    ] {
        let (status, served) = call(&router, operation, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the stored resource must still be readable, got {status} with \
             {served}"
        );
    }
}

/// What a create stored is what a describe reports: the round trip through state,
/// for each catalog resource whose lookup this suite pins.
#[tokio::test]
async fn a_created_resource_reads_back_through_its_describe() {
    let router = emulator();
    let seeded = seed(&router).await;

    let (status, constraint) = call(
        &router,
        "DescribeConstraint",
        &json!({"Id": seeded.constraint_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"constraint\": expected 200, got {status} with {constraint}"
    );
    assert_eq!(
        field("constraint", &constraint, "/ConstraintDetail/Type"),
        "NOTIFICATION",
        "case \"constraint\": the stored type must come back, got {constraint}"
    );
    assert_eq!(
        field("constraint", &constraint, "/ConstraintDetail/PortfolioId"),
        seeded.portfolio_id,
        "case \"constraint\": the stored portfolio must come back, got {constraint}"
    );
    assert_eq!(
        field("constraint", &constraint, "/ConstraintDetail/ProductId"),
        seeded.product_id,
        "case \"constraint\": the stored product must come back, got {constraint}"
    );
    assert!(
        field("constraint", &constraint, "/ConstraintParameters").contains("NotificationArns"),
        "case \"constraint\": the stored parameters must come back, got {constraint}"
    );

    // The product reads back by id and by name, and both report the same record. The
    // seeded product sits in no portfolio, so the read that reaches it is the ADMIN one;
    // the end-user `DescribeProduct` sees a product only through a launch path, which
    // the block below gives it.
    for (case, body) in [
        ("product by id", json!({"Id": seeded.product_id})),
        ("product by name", json!({"Name": seeded.product_name})),
    ] {
        let (status, product) = call(&router, "DescribeProductAsAdmin", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {product}"
        );
        assert_eq!(
            field(
                case,
                &product,
                "/ProductViewDetail/ProductViewSummary/ProductId"
            ),
            seeded.product_id,
            "case {case:?}: the stored product id must come back, got {product}"
        );
        assert_eq!(
            field(case, &product, "/ProductViewDetail/ProductViewSummary/Name"),
            seeded.product_name,
            "case {case:?}: the stored name must come back, got {product}"
        );
        assert_eq!(
            product
                .pointer("/ProvisioningArtifactSummaries/0/Id")
                .and_then(Value::as_str),
            Some(seeded.provisioning_artifact_id.as_str()),
            "case {case:?}: the stored version must be listed, got {product}"
        );
    }

    let (status, versions) = call(
        &router,
        "ListProvisioningArtifacts",
        &json!({"ProductId": seeded.product_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"versions\": expected 200, got {status} with {versions}"
    );
    assert_eq!(
        field("versions", &versions, "/ProvisioningArtifactDetails/0/Id"),
        seeded.provisioning_artifact_id,
        "case \"versions\": the version CreateProduct minted must be listed, got {versions}"
    );
    assert_eq!(
        field("versions", &versions, "/ProvisioningArtifactDetails/0/Name"),
        "v1",
        "case \"versions\": the stored version name must come back, got {versions}"
    );
    assert_eq!(
        versions
            .pointer("/ProvisioningArtifactDetails/0/Active")
            .and_then(Value::as_bool),
        Some(true),
        "case \"versions\": a created version is active, got {versions}"
    );
    assert_eq!(
        field("versions", &versions, "/ProvisioningArtifactDetails/0/Guidance"),
        "DEFAULT",
        "case \"versions\": a created version carries the default guidance, got {versions}"
    );

    // Associating the product with a portfolio gives it a launch path through that
    // portfolio, and the describe reports both halves from the same stored set.
    let (status, body) = call(
        &router,
        "AssociateProductWithPortfolio",
        &json!({"ProductId": seeded.product_id, "PortfolioId": seeded.portfolio_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"associated product\": expected 200, got {status} with {body}"
    );
    let (status, described) = call(
        &router,
        "DescribeProduct",
        &json!({"Id": seeded.product_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"associated product\": expected 200, got {status} with {described}"
    );
    assert_eq!(
        field("associated product", &described, "/LaunchPaths/0/Id"),
        seeded.portfolio_id,
        "case \"associated product\": the launch path is the associated portfolio, got {described}"
    );
    assert_eq!(
        field("associated product", &described, "/LaunchPaths/0/Name"),
        "seed-portfolio",
        "case \"associated product\": the path is named by the stored portfolio, got {described}"
    );
    assert_eq!(
        described
            .pointer("/ProductViewSummary/HasDefaultPath")
            .and_then(Value::as_bool),
        Some(true),
        "case \"associated product\": one path needs no disambiguation, so it is the default, got \
         {described}"
    );

    let (status, action) = call(
        &router,
        "DescribeServiceAction",
        &json!({"Id": seeded.service_action_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"service action\": expected 200, got {status} with {action}"
    );
    assert_eq!(
        field(
            "service action",
            &action,
            "/ServiceActionDetail/ServiceActionSummary/Name"
        ),
        "seed-action",
        "case \"service action\": the stored name must come back, got {action}"
    );
    assert_eq!(
        field("service action", &action, "/ServiceActionDetail/Definition/Name"),
        "AWS-RestartEC2Instance",
        "case \"service action\": the stored definition must come back, got {action}"
    );

    let (status, option) = call(
        &router,
        "DescribeTagOption",
        &json!({"Id": seeded.tag_option_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"tag option\": expected 200, got {status} with {option}"
    );
    assert_eq!(
        field("tag option", &option, "/TagOptionDetail/Key"),
        "seed-key",
        "case \"tag option\": the stored key must come back, got {option}"
    );
    assert_eq!(
        field("tag option", &option, "/TagOptionDetail/Value"),
        "seed-value",
        "case \"tag option\": the stored value must come back, got {option}"
    );
    assert_eq!(
        option
            .pointer("/TagOptionDetail/Active")
            .and_then(Value::as_bool),
        Some(true),
        "case \"tag option\": a created TagOption is active, got {option}"
    );
}

/// `CreateProduct` must be given a template source it can read: the request carries
/// it either as `ProvisioningArtifactParameters.Info` — whose accepted keys and
/// their forms the model documents — or as a `SourceConnection`, and a request with
/// neither, or with an `Info` that names no readable source, is refused with the
/// modelled `InvalidParametersException` rather than storing a product nothing can
/// be provisioned from.
#[tokio::test]
async fn create_product_needs_a_readable_template_source() {
    let router = emulator();

    // Rows differ along WHICH source the request carries (and along the product
    // type, so the rule is not read off one product shape). The `Info` keys are the
    // model's: "Keys accepted: [ LoadTemplateFromURL, ImportFromPhysicalId ]".
    let accepted: [(&str, Value, &str); 3] = [
        (
            "Info with LoadTemplateFromURL",
            artifact_parameters("v1"),
            "CLOUD_FORMATION_TEMPLATE",
        ),
        (
            "Info with ImportFromPhysicalId",
            json!({
                "Name": "v2",
                "Info": {"ImportFromPhysicalId": "arn:aws:cloudformation:us-east-1:000000000000:stack/s/1"},
            }),
            "CLOUD_FORMATION_TEMPLATE",
        ),        (
            "Info with an http template URL and another product type",
            json!({
                "Name": "v3",
                "Info": {"LoadTemplateFromURL": "http://example.invalid/dir/t.json"},
                "Type": "TERRAFORM_OPEN_SOURCE",
            }),
            "TERRAFORM_OPEN_SOURCE",
        ),
    ];

    for (case, parameters, product_type) in accepted {
        let (status, created) = call(
            &router,
            "CreateProduct",
            &json!({
                "Name": format!("accepted-{case}"),
                "Owner": "acme",
                "ProductType": product_type,
                "ProvisioningArtifactParameters": parameters,
                "IdempotencyToken": format!("tok-accepted-{}", case.len()),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a request carrying a template source must be served, got {status} \
             with {created}"
        );
        // The product is readable afterwards, so the create really stored it. A created
        // product sits in no portfolio, so the read that reaches it is the admin one.
        let id = field(case, &created, "/ProductViewDetail/ProductViewSummary/ProductId");
        let (status, described) =
            call(&router, "DescribeProductAsAdmin", &json!({"Id": id})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the created product must read back, got {status} with {described}"
        );
    }

    // A repo-synced product names its source instead, and is served on that alone.
    let (status, synced) = call(
        &router,
        "CreateProduct",
        &json!({
            "Name": "repo-product",
            "Owner": "acme",
            "ProductType": "EXTERNAL",
            "SourceConnection": {
                "Type": "CODESTAR",
                "ConnectionParameters": {"CodeStar": {
                    "ConnectionArn": "arn:aws:codestar-connections:us-east-1:000000000000:connection/abc",
                    "Repository": "acme/templates",
                    "Branch": "main",
                    "ArtifactPath": "folder/file.json",
                }},
            },
            "IdempotencyToken": "tok-repo",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"SourceConnection only\": expected 200, got {status} with {synced}"
    );

    // Rows differ along HOW the source is missing or unusable: no parameters member
    // at all, a parameters member with no `Info`, a parameters member carrying only
    // the other properties, an `Info` naming a key the model does not accept, and a
    // `LoadTemplateFromURL` that is no URL at all.
    let refused_rows: [(&str, Option<Value>); 5] = [
        ("no ProvisioningArtifactParameters", None),
        ("parameters with no Info", Some(json!({"Name": "v1"}))),
        (
            "parameters with name, description and type but no Info",
            Some(json!({
                "Name": "v1",
                "Description": "no template",
                "Type": "CLOUD_FORMATION_TEMPLATE",
            })),
        ),
        (
            "Info with a key the model does not accept",
            Some(json!({"Name": "v1", "Info": {"TemplateBody": "{}"}})),
        ),
        (
            "LoadTemplateFromURL that is not a URL",
            Some(json!({"Name": "v1", "Info": {"LoadTemplateFromURL": "template.json"}})),
        ),
    ];

    for (case, parameters) in refused_rows {
        let mut body = json!({
            "Name": format!("refused-{case}"),
            "Owner": "acme",
            "ProductType": "CLOUD_FORMATION_TEMPLATE",
            "IdempotencyToken": format!("tok-refused-{}", case.len()),
        });
        if let Some(parameters) = parameters {
            body["ProvisioningArtifactParameters"] = parameters;
        }
        let (status, refused) = call(&router, "CreateProduct", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidParametersException",
            "case {case:?}: unexpected error in {refused}"
        );
        // The refusal stored nothing: the product it would have made is unknown.
        let (status, described) = call(
            &router,
            "DescribeProduct",
            &json!({"Name": format!("refused-{case}")}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a refused create must store nothing, got {status} with {described}"
        );
        assert_eq!(
            error_code(&described),
            "ResourceNotFoundException",
            "case {case:?}: a refused create must store nothing, got {described}"
        );
    }
}

/// `CreateConstraint` answers in the non-terminal `CREATING` status, so the move to
/// the terminal `AVAILABLE` is owed by the clock: a caller acting straight after the
/// create still sees `CREATING`, and a later read sees `AVAILABLE` — without any
/// caller having to poll for it.
#[tokio::test]
async fn a_created_constraint_moves_from_creating_to_available() {
    let router = emulator();
    let seeded = seed(&router).await;

    // A second constraint, created here so its due instant is this test's.
    let (status, created) = call(
        &router,
        "CreateConstraint",
        &json!({
            "PortfolioId": seeded.portfolio_id,
            "ProductId": seeded.product_id,
            "Type": "LAUNCH",
            "Parameters": r#"{"LocalRoleName":"SCBasicLaunchRole"}"#,
            "IdempotencyToken": "tok-clock",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"create\": expected 200, got {status} with {created}"
    );
    assert_eq!(
        field("create", &created, "/Status"),
        "CREATING",
        "case \"create\": creation has started, so the status is the non-terminal one, got \
         {created}"
    );
    let id = field("create", &created, "/ConstraintDetail/ConstraintId").to_string();

    // Straight after the create, the transient status is still what a reader sees.
    let (status, immediately) = call(&router, "DescribeConstraint", &json!({"Id": id})).await;
    assert_eq!(
        status, 200,
        "case \"immediately\": expected 200, got {status} with {immediately}"
    );
    assert_eq!(
        field("immediately", &immediately, "/Status"),
        "CREATING",
        "case \"immediately\": the transition is not due yet, got {immediately}"
    );

    // Once the clock owes the transition, every reader sees the terminal status —
    // nothing had to ask for it.
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    let (status, later) = call(&router, "DescribeConstraint", &json!({"Id": id})).await;
    assert_eq!(
        status, 200,
        "case \"after the due instant\": expected 200, got {status} with {later}"
    );
    assert_eq!(
        field("after the due instant", &later, "/Status"),
        "AVAILABLE",
        "case \"after the due instant\": the clock owed the transition, got {later}"
    );
    // And it is the same constraint throughout.
    assert_eq!(
        field("after the due instant", &later, "/ConstraintDetail/Type"),
        "LAUNCH",
        "case \"after the due instant\": the stored type must not change, got {later}"
    );
}

/// A TagOption IS its key/value pair, so creating a pair that already exists is the
/// modelled `DuplicateResourceException`; any other pair is a new TagOption with its
/// own id.
#[tokio::test]
async fn create_tag_option_refuses_a_pair_that_already_exists() {
    let router = emulator();

    // Rows differ along what the second create changes: nothing (the duplicate),
    // only the value, only the key, and the letter case of the value — the stored
    // pair is the exact strings, so a differing case is a different pair.
    let rows: [(&str, &str, &str, bool); 5] = [
        ("first create", "CostCenter", "1234", false),
        ("same key and value", "CostCenter", "1234", true),
        ("same key, other value", "CostCenter", "5678", false),
        ("other key, same value", "Owner", "1234", false),
        ("value differing in case", "CostCenter", "ABCD", false),
    ];

    let mut minted: Vec<String> = Vec::new();
    for (case, key, value, expect_duplicate) in rows {
        let (status, body) = call(&router, "CreateTagOption", &json!({"Key": key, "Value": value}))
            .await;
        if expect_duplicate {
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "DuplicateResourceException",
                "case {case:?}: unexpected error in {body}"
            );
            continue;
        }
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        let id = field(case, &body, "/TagOptionDetail/Id").to_string();
        assert!(
            !minted.contains(&id),
            "case {case:?}: a distinct pair must get its own id, but {id} was already minted"
        );
        // The pair is readable under the id the create minted.
        let (status, described) = call(&router, "DescribeTagOption", &json!({"Id": id})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the created TagOption must read back, got {status} with {described}"
        );
        assert_eq!(
            field(case, &described, "/TagOptionDetail/Value"),
            value,
            "case {case:?}: the stored value must come back, got {described}"
        );
        minted.push(id);
    }

    assert_eq!(
        minted.len(),
        4,
        "four distinct pairs must have been stored, got {minted:?}"
    );
}

/// `CreatePortfolio.DisplayName` targets `PortfolioDisplayName`, whose modelled
/// `@length` is min 1 max 100 (`smithy_query query="operation"
/// operation="CreatePortfolio"`): a value outside that bound is refused with HTTP
/// 400 `ValidationException` naming the member, and one inside it is served.
#[tokio::test]
async fn create_portfolio_refuses_a_display_name_outside_the_modelled_length() {
    let router = emulator();

    // Rows: each published bound, one inside, and values past the upper bound —
    // including the 103 characters the recorded case sent.
    let rows: [(&str, usize, bool); 6] = [
        ("empty, under the min bound", 0, false),
        ("at the min bound", 1, true),
        ("inside the bounds", 50, true),
        ("at the max bound", 100, true),
        ("one past the max bound", 101, false),
        ("well past the max bound", 103, false),
    ];

    for (case, length, accepted) in rows {
        let display_name = "d".repeat(length);
        let (status, body) = call(
            &router,
            "CreatePortfolio",
            &json!({
                "DisplayName": display_name,
                "ProviderName": "acme",
                "IdempotencyToken": format!("tok-len-{length}"),
            }),
        )
        .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: a name of {length} characters is inside the bounds, got {status} \
                 with {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: a name of {length} characters is outside the bounds, got {status} \
             with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: unexpected error in {body}"
        );
        let message = body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: no message in {body}"));
        assert!(
            message.contains("displayName"),
            "case {case:?}: the refusal must name the offending member, got {message:?}"
        );
    }
}

/// `ListPrincipalsForPortfolio` reports the associations stored ON the portfolio: a
/// portfolio with none lists none, each association is reported once per ARN, and
/// the page token walks the stored order.
#[tokio::test]
async fn principals_are_listed_from_the_associations_stored_on_the_portfolio() {
    let router = emulator();
    let seeded = seed(&router).await;

    // A portfolio nobody has associated with lists an empty set — not a failure.
    let (status, empty) = call(
        &router,
        "ListPrincipalsForPortfolio",
        &json!({"PortfolioId": seeded.portfolio_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"no associations\": expected 200, got {status} with {empty}"
    );
    assert_eq!(
        empty.pointer("/Principals").and_then(Value::as_array),
        Some(&Vec::new()),
        "case \"no associations\": expected an empty Principals list, got {empty}"
    );

    // Rows differ along the principal type (the modelled `IAM` | `IAM_PATTERN`) and
    // along the ARN form each type admits.
    let associations: [(&str, &str, &str); 3] = [
        (
            "a role ARN",
            "arn:aws:iam::000000000000:role/Admin",
            "IAM",
        ),
        (
            "a user ARN",
            "arn:aws:iam::000000000000:user/Builder",
            "IAM",
        ),
        (
            "an account-less pattern",
            "arn:aws:iam:::role/Team*",
            "IAM_PATTERN",
        ),
    ];

    for (case, arn, principal_type) in associations {
        let (status, body) = call(
            &router,
            "AssociatePrincipalWithPortfolio",
            &json!({
                "PortfolioId": seeded.portfolio_id,
                "PrincipalARN": arn,
                "PrincipalType": principal_type,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
    }

    // Re-associating one ARN changes its type rather than listing the ARN twice. Under
    // `IAM_PATTERN` the value reported for it is the ARN itself, which is how the row is
    // found below: a pattern names no single principal to resolve.
    let (status, body) = call(
        &router,
        "AssociatePrincipalWithPortfolio",
        &json!({
            "PortfolioId": seeded.portfolio_id,
            "PrincipalARN": "arn:aws:iam::000000000000:user/Builder",
            "PrincipalType": "IAM_PATTERN",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"re-associate\": expected 200, got {status} with {body}"
    );

    let (status, listed) = call(
        &router,
        "ListPrincipalsForPortfolio",
        &json!({"PortfolioId": seeded.portfolio_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"all associations\": expected 200, got {status} with {listed}"
    );
    let principals = listed
        .pointer("/Principals")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case \"all associations\": no Principals in {listed}"));
    assert_eq!(
        principals.len(),
        3,
        "case \"all associations\": three ARNs were associated, got {listed}"
    );
    let builder = principals
        .iter()
        .find(|principal| {
            principal["PrincipalARN"] == json!("arn:aws:iam::000000000000:user/Builder")
        })
        .unwrap_or_else(|| panic!("case \"re-associate\": the ARN is not listed in {listed}"));
    assert_eq!(
        builder["PrincipalType"],
        json!("IAM_PATTERN"),
        "case \"re-associate\": the stored type must be the last one associated, got {builder}"
    );

    // `PageSize` bounds one page and the token resumes the walk; the pages together
    // are the stored set, with nothing repeated.
    let (status, first) = call(
        &router,
        "ListPrincipalsForPortfolio",
        &json!({"PortfolioId": seeded.portfolio_id, "PageSize": 2}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"first page\": expected 200, got {status} with {first}"
    );
    let first_page = first
        .pointer("/Principals")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case \"first page\": no Principals in {first}"));
    assert_eq!(
        first_page.len(),
        2,
        "case \"first page\": PageSize 2 bounds the page, got {first}"
    );
    let token = field("first page", &first, "/NextPageToken").to_string();

    let (status, second) = call(
        &router,
        "ListPrincipalsForPortfolio",
        &json!({"PortfolioId": seeded.portfolio_id, "PageSize": 2, "PageToken": token}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"second page\": expected 200, got {status} with {second}"
    );
    let second_page = second
        .pointer("/Principals")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case \"second page\": no Principals in {second}"));
    assert_eq!(
        second_page.len(),
        1,
        "case \"second page\": one association remains, got {second}"
    );
    assert!(
        second.get("NextPageToken").is_none()
            || second["NextPageToken"] == Value::Null,
        "case \"second page\": the walk is finished, so no cursor is handed back, got {second}"
    );
    assert!(
        !first_page.contains(&second_page[0]),
        "case \"second page\": a page must not repeat the previous one: {first} then {second}"
    );

    // A token this service did not mint is a bad parameter, not an empty page.
    let (status, refused) = call(
        &router,
        "ListPrincipalsForPortfolio",
        &json!({"PortfolioId": seeded.portfolio_id, "PageToken": "not-a-token"}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"foreign page token\": expected 400, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "InvalidParametersException",
        "case \"foreign page token\": unexpected error in {refused}"
    );
}

/// A principal ARN must be one the requested `PrincipalType` admits: an `IAM`
/// principal is a fully defined IAM ARN of a user, role or group in THIS account,
/// while an `IAM_PATTERN` may omit the account id and may carry wildcards. A value
/// that is no such ARN is the modelled `InvalidParametersException`; a well-formed
/// ARN of another account names no principal here, which is the modelled
/// `ResourceNotFoundException`.
#[tokio::test]
async fn a_principal_arn_the_type_does_not_admit_is_refused() {
    let router = emulator();
    let seeded = seed(&router).await;

    // Rows differ along the ARN's shape and the type it is offered under. `None`
    // means the association is served; otherwise it is the expected error code.
    let rows: [(&str, &str, &str, Option<&str>); 10] = [
        (
            "a fully defined role ARN as IAM",
            "arn:aws:iam::000000000000:role/Admin",
            "IAM",
            None,
        ),
        (
            "a group ARN as IAM",
            "arn:aws:iam::000000000000:group/Builders",
            "IAM",
            None,
        ),
        (
            "a wildcard pattern with no account as IAM_PATTERN",
            "arn:aws:iam:::role/Team*",
            "IAM_PATTERN",
            None,
        ),
        (
            "a fully defined ARN as IAM_PATTERN",
            "arn:aws:iam::000000000000:role/Exact",
            "IAM_PATTERN",
            None,
        ),
        ("not an ARN at all", "Admin", "IAM", Some("InvalidParametersException")),
        (
            "an ARN of another service",
            "arn:aws:sts::000000000000:role/Admin",
            "IAM",
            Some("InvalidParametersException"),
        ),
        (
            "an IAM ARN carrying a region",
            "arn:aws:iam:us-east-1:000000000000:role/Admin",
            "IAM",
            Some("InvalidParametersException"),
        ),
        (
            "an ARN naming no principal kind",
            "arn:aws:iam::000000000000:policy/Admin",
            "IAM",
            Some("InvalidParametersException"),
        ),
        (
            "an account-less ARN as IAM",
            "arn:aws:iam:::role/Admin",
            "IAM",
            Some("InvalidParametersException"),
        ),
        (
            "a principal of another account",
            "arn:aws:iam::111111111111:role/Foreign",
            "IAM",
            Some("ResourceNotFoundException"),
        ),
    ];

    for (case, arn, principal_type, expected) in rows {
        let (status, body) = call(
            &router,
            "AssociatePrincipalWithPortfolio",
            &json!({
                "PortfolioId": seeded.portfolio_id,
                "PrincipalARN": arn,
                "PrincipalType": principal_type,
            }),
        )
        .await;
        match expected {
            None => assert_eq!(
                status, 200,
                "case {case:?}: {arn} must be associable as {principal_type}, got {status} with \
                 {body}"
            ),
            Some(code) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected 400 for {arn}, got {status} with {body}"
                );
                assert_eq!(
                    error_code(&body),
                    code,
                    "case {case:?}: unexpected error for {arn} in {body}"
                );
            }
        }
    }

    // Only the four served ARNs were stored.
    let (_, listed) = call(
        &router,
        "ListPrincipalsForPortfolio",
        &json!({"PortfolioId": seeded.portfolio_id}),
    )
    .await;
    assert_eq!(
        listed
            .pointer("/Principals")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(4),
        "the refused associations must store nothing, got {listed}"
    );

    // What the FIRST association — the role associated as `IAM` — is reported as, read
    // before it is removed. A concrete principal is reported as its identifier rather
    // than as the ARN the request carried, so this is the entry the disassociation has to
    // take away; the associations are reported in the order they were made, so it is the
    // first.
    let removed = listed
        .pointer("/Principals/0")
        .cloned()
        .unwrap_or_else(|| panic!("no first Principal in {listed}"));

    // And a disassociation removes the one it names, leaving the rest.
    let (status, body) = call(
        &router,
        "DisassociatePrincipalFromPortfolio",
        &json!({
            "PortfolioId": seeded.portfolio_id,
            "PrincipalARN": "arn:aws:iam::000000000000:role/Admin",
            "PrincipalType": "IAM",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"disassociate\": expected 200, got {status} with {body}"
    );
    let (_, listed) = call(
        &router,
        "ListPrincipalsForPortfolio",
        &json!({"PortfolioId": seeded.portfolio_id}),
    )
    .await;
    let remaining = listed
        .pointer("/Principals")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case \"disassociate\": no Principals in {listed}"));
    assert_eq!(
        remaining.len(),
        3,
        "case \"disassociate\": exactly the named association is gone, got {listed}"
    );
    assert!(
        !remaining.contains(&removed),
        "case \"disassociate\": the named association must be gone, got {listed}"
    );
}

/// A resource a delete removed is a resource no later call can find: the describe
/// that answered 200 before answers the modelled 400 `ResourceNotFoundException`
/// afterwards, and the list it appeared in no longer carries it.
#[tokio::test]
async fn a_deleted_resource_is_resource_not_found() {
    let router = emulator();
    let seeded = seed(&router).await;

    // Rows: one per resource kind whose lookup this suite pins, with the describe
    // that reads it, the delete that removes it, and the id of the seeded record.
    let rows: [(&str, &str, &str, &str); 4] = [
        (
            "constraint",
            "DescribeConstraint",
            "DeleteConstraint",
            &seeded.constraint_id,
        ),
        (
            "service action",
            "DescribeServiceAction",
            "DeleteServiceAction",
            &seeded.service_action_id,
        ),
        (
            "tag option",
            "DescribeTagOption",
            "DeleteTagOption",
            &seeded.tag_option_id,
        ),
        (
            "product",
            "DescribeProductAsAdmin",
            "DeleteProduct",
            &seeded.product_id,
        ),
    ];

    for (case, describe, delete, id) in rows {
        let (status, before) = call(&router, describe, &json!({"Id": id})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the seeded record must be readable first, got {status} with {before}"
        );

        let (status, deleted) = call(&router, delete, &json!({"Id": id})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the delete to be served, got {status} with {deleted}"
        );

        let (status, after) = call(&router, describe, &json!({"Id": id})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a deleted record must not be readable, got {status} with {after}"
        );
        assert_eq!(
            error_code(&after),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error after the delete in {after}"
        );

        // Deleting it twice is the same refusal, not a second success.
        let (status, again) = call(&router, delete, &json!({"Id": id})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a second delete must be refused, got {status} with {again}"
        );
        assert_eq!(
            error_code(&again),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error on the second delete in {again}"
        );
    }

    // The lists report the stored set, which is now empty of all of them.
    for (case, operation, pointer, body) in [
        ("tag options", "ListTagOptions", "/TagOptionDetails", json!({})),
        (
            "service actions",
            "ListServiceActions",
            "/ServiceActionSummaries",
            json!({}),
        ),
        (
            "constraints",
            "ListConstraintsForPortfolio",
            "/ConstraintDetails",
            json!({"PortfolioId": seeded.portfolio_id}),
        ),
    ] {
        let (status, listed) = call(&router, operation, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {listed}"
        );
        assert_eq!(
            listed.pointer(pointer).and_then(Value::as_array).map(Vec::len),
            Some(0),
            "case {case:?}: every record of this kind was deleted, got {listed}"
        );
    }
}

/// A portfolio that still carries an association cannot be deleted: AWS refuses it
/// with the modelled `ResourceInUseException` ("You cannot delete a portfolio if it
/// was shared with you or if it has associated products, users, constraints, or
/// shared accounts"), and the delete succeeds once the associations are cleared.
#[tokio::test]
async fn a_portfolio_in_use_cannot_be_deleted_until_it_is_cleared() {
    let router = emulator();
    let seeded = seed(&router).await;

    // The seeded portfolio already carries a constraint; add a product association
    // and a principal, so each kind of association is exercised in turn.
    let (status, body) = call(
        &router,
        "AssociateProductWithPortfolio",
        &json!({"ProductId": seeded.product_id, "PortfolioId": seeded.portfolio_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"associate product\": expected 200, got {status} with {body}"
    );
    let (status, body) = call(
        &router,
        "AssociatePrincipalWithPortfolio",
        &json!({
            "PortfolioId": seeded.portfolio_id,
            "PrincipalARN": "arn:aws:iam::000000000000:role/Admin",
            "PrincipalType": "IAM",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"associate principal\": expected 200, got {status} with {body}"
    );

    // Rows: the delete is refused while any association remains, whichever kind it
    // is, and each row clears one kind before trying again.
    let clearing: [(&str, &str, Value); 3] = [
        (
            "while a product is associated",
            "DeleteConstraint",
            json!({"Id": seeded.constraint_id}),
        ),
        (
            "while the product association remains",
            "DisassociateProductFromPortfolio",
            json!({"ProductId": seeded.product_id, "PortfolioId": seeded.portfolio_id}),
        ),
        (
            "while a principal is associated",
            "DisassociatePrincipalFromPortfolio",
            json!({
                "PortfolioId": seeded.portfolio_id,
                "PrincipalARN": "arn:aws:iam::000000000000:role/Admin",
                "PrincipalType": "IAM",
            }),
        ),
    ];

    for (case, clear, clear_body) in clearing {
        let (status, refused) = call(
            &router,
            "DeletePortfolio",
            &json!({"Id": seeded.portfolio_id}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceInUseException",
            "case {case:?}: unexpected error in {refused}"
        );
        // The refused delete left the portfolio readable.
        let (status, still_there) = call(
            &router,
            "DescribePortfolio",
            &json!({"Id": seeded.portfolio_id}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a refused delete must leave the portfolio, got {status} with \
             {still_there}"
        );

        let (status, cleared) = call(&router, clear, &clear_body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {clear} to be served, got {status} with {cleared}"
        );
    }

    // Nothing is associated any more, so the delete is served and the portfolio is
    // gone for good.
    let (status, deleted) = call(
        &router,
        "DeletePortfolio",
        &json!({"Id": seeded.portfolio_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"cleared\": expected 200, got {status} with {deleted}"
    );
    let (status, gone) = call(
        &router,
        "DescribePortfolio",
        &json!({"Id": seeded.portfolio_id}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"cleared\": a deleted portfolio must not be readable, got {status} with {gone}"
    );
    assert_eq!(
        error_code(&gone),
        "ResourceNotFoundException",
        "case \"cleared\": unexpected error in {gone}"
    );
}

/// An update changes only the members it names, and what it stored is what the
/// describe beside it reports.
#[tokio::test]
async fn an_update_is_visible_through_the_describe_beside_it() {
    let router = emulator();
    let seeded = seed(&router).await;

    // TagOption: the value moves, the key does not.
    let (status, updated) = call(
        &router,
        "UpdateTagOption",
        &json!({"Id": seeded.tag_option_id, "Value": "seed-value-2"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"tag option\": expected 200, got {status} with {updated}"
    );
    let (_, described) = call(
        &router,
        "DescribeTagOption",
        &json!({"Id": seeded.tag_option_id}),
    )
    .await;
    assert_eq!(
        field("tag option", &described, "/TagOptionDetail/Value"),
        "seed-value-2",
        "case \"tag option\": the stored value must be the updated one, got {described}"
    );
    assert_eq!(
        field("tag option", &described, "/TagOptionDetail/Key"),
        "seed-key",
        "case \"tag option\": an unnamed member keeps its stored value, got {described}"
    );

    // Moving a TagOption onto a pair another one already holds is the duplicate.
    let (status, other) = call(
        &router,
        "CreateTagOption",
        &json!({"Key": "seed-key", "Value": "taken"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"second pair\": expected 200, got {status} with {other}"
    );
    let (status, refused) = call(
        &router,
        "UpdateTagOption",
        &json!({"Id": seeded.tag_option_id, "Value": "taken"}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"duplicate update\": expected 400, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "DuplicateResourceException",
        "case \"duplicate update\": unexpected error in {refused}"
    );
    let (_, unchanged) = call(
        &router,
        "DescribeTagOption",
        &json!({"Id": seeded.tag_option_id}),
    )
    .await;
    assert_eq!(
        field("duplicate update", &unchanged, "/TagOptionDetail/Value"),
        "seed-value-2",
        "case \"duplicate update\": a refused update must store nothing, got {unchanged}"
    );

    // Service action: the description moves, the definition type does not.
    let (status, updated) = call(
        &router,
        "UpdateServiceAction",
        &json!({"Id": seeded.service_action_id, "Description": "restarts an instance"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"service action\": expected 200, got {status} with {updated}"
    );
    let (_, described) = call(
        &router,
        "DescribeServiceAction",
        &json!({"Id": seeded.service_action_id}),
    )
    .await;
    assert_eq!(
        field(
            "service action",
            &described,
            "/ServiceActionDetail/ServiceActionSummary/Description"
        ),
        "restarts an instance",
        "case \"service action\": the stored description must be the updated one, got {described}"
    );
    assert_eq!(
        field(
            "service action",
            &described,
            "/ServiceActionDetail/ServiceActionSummary/DefinitionType"
        ),
        "SSM_AUTOMATION",
        "case \"service action\": the definition type is not updatable, got {described}"
    );

    // Constraint: the parameters move, and a value that is no JSON object is refused
    // without changing what is stored.
    let (status, updated) = call(
        &router,
        "UpdateConstraint",
        &json!({
            "Id": seeded.constraint_id,
            "Parameters": r#"{"NotificationArns":["arn:aws:sns:us-east-1:000000000000:other"]}"#,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"constraint\": expected 200, got {status} with {updated}"
    );
    let (status, refused) = call(
        &router,
        "UpdateConstraint",
        &json!({"Id": seeded.constraint_id, "Parameters": "not json"}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"constraint parameters\": expected 400, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "InvalidParametersException",
        "case \"constraint parameters\": unexpected error in {refused}"
    );
    let (_, described) = call(
        &router,
        "DescribeConstraint",
        &json!({"Id": seeded.constraint_id}),
    )
    .await;
    assert!(
        field("constraint", &described, "/ConstraintParameters").contains("other"),
        "case \"constraint\": the stored parameters must be the updated ones, got {described}"
    );

    // Product: the name moves, and the describe-by-name follows it.
    let (status, updated) = call(
        &router,
        "UpdateProduct",
        &json!({"Id": seeded.product_id, "Name": "renamed-product"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"product\": expected 200, got {status} with {updated}"
    );
    let (status, described) = call(
        &router,
        "DescribeProductAsAdmin",
        &json!({"Name": "renamed-product"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"product\": the renamed product must be findable by its new name, got {status} \
         with {described}"
    );
    assert_eq!(
        field(
            "product",
            &described,
            "/ProductViewDetail/ProductViewSummary/ProductId"
        ),
        seeded.product_id,
        "case \"product\": it is the same product, got {described}"
    );
    assert_eq!(
        field(
            "product",
            &described,
            "/ProductViewDetail/ProductViewSummary/Owner"
        ),
        "acme",
        "case \"product\": an unnamed member keeps its stored value, got {described}"
    );
}
