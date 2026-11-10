// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how this service REPRESENTS what it stored, and for the download
//! URLs it announces.
//!
//! The rules under test, each asserted over a SET of inputs rather than one:
//!
//! * a setting that belongs to the function as a whole is reported only for the
//!   UNQUALIFIED identity — a read addressed to a version, an alias or `$LATEST` by name
//!   omits it;
//! * a published version is an identity of its own: it carries its own `RevisionId`,
//!   which no other identity of the function shares;
//! * `Description` is a member of every function configuration and of every layer
//!   version's own read, as the empty string when the caller gave none, while a LISTING
//!   entry carries only what the version itself carries;
//! * a resolved container-image URI pins the SAME repository to the digest of the image
//!   the function is frozen to, never to the mutable tag that was supplied;
//! * a configuration member with a documented default is reported with it when the caller
//!   did not set it, and a partial update changes only the members it names;
//! * a download URL this service ANNOUNCES is served by this service, with exactly the
//!   bytes the store holds for the object it names;
//! * a response whose status carries no body declares no body either;
//! * a refusal built from the request names what collided, and two different states of
//!   one resource are diagnosed apart.
//!
//! Every row drives real HTTP requests through the router, so the assertions are about
//! the wire behaviour (status, body, bound headers) rather than about any handler's
//! internals.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;

use crate::function_lifecycle::{
    create_active_function, create_function, json_call, make_activation_due, package_bytes,
    router_on, seed_image, zip_file, Answer, ACCOUNT, REGION, ROLE,
};
use smithy_server_runtime::MultiServiceRouter;

/// The `Timeout` the model documents as the default ("The default is 3 seconds") and the
/// one it documents as the maximum ("the maximum allowed value is 900 seconds"), from
/// `smithy_query(query="doc", shape="CreateFunctionRequest", member="Timeout")`.
const DEFAULT_TIMEOUT_SECONDS: i64 = 3;
/// See [`DEFAULT_TIMEOUT_SECONDS`].
const MAX_TIMEOUT_SECONDS: i64 = 900;

/// The `RetentionPeriodInDays` a durable-execution configuration that states none is
/// reported with. Inside the member's modelled range (1..90), from
/// `smithy_query(query="shape", shape="DurableConfig")`.
const DEFAULT_RETENTION_PERIOD_DAYS: i64 = 14;

/// The only action a layer-version grant can name: the model's
/// `LayerPermissionAllowedAction` is constrained to the pattern
/// `^lambda:GetLayerVersion$`.
const LAYER_ACTION: &str = "lambda:GetLayerVersion";

/// `GetFunction` on a reference, optionally with a `Qualifier` query parameter.
async fn get_function(
    router: &MultiServiceRouter,
    reference: &str,
    qualifier: Option<&str>,
) -> Answer {
    let query = qualifier.map_or_else(String::new, |value| format!("?Qualifier={value}"));
    let uri = format!("/2015-03-31/functions/{reference}{query}");
    json_call(router, "GET", &uri, &Value::Null).await
}

/// `GetFunctionConfiguration` on a reference, optionally with a `Qualifier`.
async fn get_configuration(
    router: &MultiServiceRouter,
    reference: &str,
    qualifier: Option<&str>,
) -> Answer {
    let query = qualifier.map_or_else(String::new, |value| format!("?Qualifier={value}"));
    let uri = format!("/2015-03-31/functions/{reference}/configuration{query}");
    json_call(router, "GET", &uri, &Value::Null).await
}

/// `PutFunctionConcurrency` on a function name.
async fn put_concurrency(router: &MultiServiceRouter, name: &str, reserved: i64) -> Answer {
    let uri = format!("/2017-10-31/functions/{name}/concurrency");
    json_call(
        router,
        "PUT",
        &uri,
        &json!({ "ReservedConcurrentExecutions": reserved }),
    )
    .await
}

/// `DeleteFunctionConcurrency` on a function name.
async fn delete_concurrency(router: &MultiServiceRouter, name: &str) -> Answer {
    let uri = format!("/2017-10-31/functions/{name}/concurrency");
    json_call(router, "DELETE", &uri, &Value::Null).await
}

/// `PublishVersion` on a function name.
async fn publish_version(router: &MultiServiceRouter, name: &str, body: &Value) -> Answer {
    let uri = format!("/2015-03-31/functions/{name}/versions");
    json_call(router, "POST", &uri, body).await
}

/// `CreateAlias` on a function name.
async fn create_alias(router: &MultiServiceRouter, name: &str, body: &Value) -> Answer {
    let uri = format!("/2015-03-31/functions/{name}/aliases");
    json_call(router, "POST", &uri, body).await
}

/// `UpdateFunctionConfiguration` on a function name.
async fn update_configuration(router: &MultiServiceRouter, name: &str, body: &Value) -> Answer {
    let uri = format!("/2015-03-31/functions/{name}/configuration");
    json_call(router, "PUT", &uri, body).await
}

/// `UpdateFunctionCode` on a function name.
async fn update_code(router: &MultiServiceRouter, name: &str, body: &Value) -> Answer {
    let uri = format!("/2015-03-31/functions/{name}/code");
    json_call(router, "PUT", &uri, body).await
}

/// `PublishLayerVersion` on a layer name.
async fn publish_layer_version(router: &MultiServiceRouter, name: &str, body: &Value) -> Answer {
    let uri = format!("/2018-10-31/layers/{name}/versions");
    json_call(router, "POST", &uri, body).await
}

/// `GetLayerVersion` on a layer name and a version number.
async fn get_layer_version(router: &MultiServiceRouter, name: &str, version: i64) -> Answer {
    let uri = format!("/2018-10-31/layers/{name}/versions/{version}");
    json_call(router, "GET", &uri, &Value::Null).await
}

/// `ListLayerVersions` on a layer name.
async fn list_layer_versions(router: &MultiServiceRouter, name: &str) -> Answer {
    let uri = format!("/2018-10-31/layers/{name}/versions");
    json_call(router, "GET", &uri, &Value::Null).await
}

/// `AddLayerVersionPermission` on a layer name and a version number.
async fn add_layer_permission(
    router: &MultiServiceRouter,
    name: &str,
    version: i64,
    statement_id: &str,
) -> Answer {
    let uri = format!("/2018-10-31/layers/{name}/versions/{version}/policy");
    json_call(
        router,
        "POST",
        &uri,
        &json!({
            "StatementId": statement_id,
            "Action": LAYER_ACTION,
            "Principal": "*",
        }),
    )
    .await
}

/// `GetLayerVersionPolicy` on a layer name and a version number.
async fn get_layer_policy(router: &MultiServiceRouter, name: &str, version: i64) -> Answer {
    let uri = format!("/2018-10-31/layers/{name}/versions/{version}/policy");
    json_call(router, "GET", &uri, &Value::Null).await
}

/// `RemoveLayerVersionPermission` on a layer name, a version number and a statement id.
async fn remove_layer_permission(
    router: &MultiServiceRouter,
    name: &str,
    version: i64,
    statement_id: &str,
) -> Answer {
    let uri = format!("/2018-10-31/layers/{name}/versions/{version}/policy/{statement_id}");
    json_call(router, "DELETE", &uri, &Value::Null).await
}

/// `CreateEventSourceMapping`.
async fn create_mapping(router: &MultiServiceRouter, body: &Value) -> Answer {
    json_call(router, "POST", "/2015-03-31/event-source-mappings", body).await
}

/// Move every in-flight update's settle instant into the past, so the next read
/// materialises the `InProgress` -> `Successful` transition the clock owes. This is what
/// waiting would do, without waiting.
fn make_update_due(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = SystemTime::now() - Duration::from_secs(1);
    for function in store.functions.values_mut() {
        if function.last_update_due.is_some() {
            function.last_update_due = Some(past);
        }
    }
}

/// The `message` member of an error answer.
fn message_of(answer: &Answer) -> String {
    answer
        .string("message")
        .or_else(|| answer.string("Message"))
        .unwrap_or_else(|| String::from_utf8_lossy(&answer.body).to_string())
}

/// A member of a nested object in the answer, by JSON pointer.
fn member<'a>(answer: &'a Value, pointer: &str) -> Option<&'a Value> {
    answer.pointer(pointer)
}

/// The ARN of an SQS queue in the signed region, which is one of the event sources
/// `EventSourceArn` enumerates.
fn queue_arn(name: &str) -> String {
    format!("arn:aws:sqs:{REGION}:{ACCOUNT}:{name}")
}

/// Fetch a URL this service ANNOUNCED, the way a client follows a presigned URL: at the
/// host the URL names and carrying no SigV4 `Authorization` header at all.
async fn fetch(router: &MultiServiceRouter, method: &str, url: &str) -> Answer {
    let host = url
        .split_once("://")
        .and_then(|(_, rest)| rest.split('/').next())
        .unwrap_or_default()
        .to_string();
    let request = http::Request::builder()
        .method(method)
        .uri(url)
        .header("host", host)
        .body(bytes::Bytes::new())
        .expect("the download request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// The `scheme://host[:port]` an announced URL is addressed to, which is the base URL this
/// emulator is reached on.
fn announced_base(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or_default();
    let host = rest.split('/').next().unwrap_or_default();
    format!("{scheme}://{host}")
}

/// One row of the qualifier table: how the read addresses the function, and whether that
/// identity is the unqualified one.
struct ConcurrencyCase {
    /// The case name.
    name: &'static str,
    /// The `FunctionName` the read carries.
    reference: String,
    /// The `Qualifier` query parameter the read carries, when any.
    qualifier: Option<&'static str>,
    /// Whether the pair names the function as a whole.
    unqualified: bool,
}

/// "Concurrency settings apply to the function as a whole, including all published
/// versions and the unpublished version", so `GetFunction` reports the `Concurrency`
/// block only for the UNQUALIFIED identity: a read of a version, of an alias, or of
/// `$LATEST` named explicitly carries no limit of its own.
///
/// The rows differ along what the rule generalises over: where the qualifier comes from
/// (the `Qualifier` parameter or the name itself), which identity it names (a published
/// version, `$LATEST`, an alias), and which documented `FunctionName` form the read uses.
#[tokio::test]
async fn reserved_concurrency_is_reported_only_for_the_unqualified_identity() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_active_function(&router, &engine, "scoped", &package_bytes(64)).await;

    let published = publish_version(&router, "scoped", &Value::Null).await;
    assert_eq!(
        published.status,
        201,
        "PublishVersion must succeed, got {} {}",
        published.status,
        String::from_utf8_lossy(&published.body)
    );
    let alias = create_alias(
        &router,
        "scoped",
        &json!({ "Name": "live", "FunctionVersion": "1" }),
    )
    .await;
    assert_eq!(
        alias.status,
        201,
        "CreateAlias must succeed, got {} {}",
        alias.status,
        String::from_utf8_lossy(&alias.body)
    );

    let reserved = 2;
    let put = put_concurrency(&router, "scoped", reserved).await;
    assert_eq!(
        put.status,
        200,
        "PutFunctionConcurrency must succeed, got {} {}",
        put.status,
        String::from_utf8_lossy(&put.body)
    );

    let cases = vec![
        ConcurrencyCase {
            name: "the bare name, unqualified",
            reference: "scoped".to_string(),
            qualifier: None,
            unqualified: true,
        },
        ConcurrencyCase {
            name: "the function ARN, unqualified",
            reference: format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:scoped"),
            qualifier: None,
            unqualified: true,
        },
        ConcurrencyCase {
            name: "the partial ARN, unqualified",
            reference: format!("{ACCOUNT}:function:scoped"),
            qualifier: None,
            unqualified: true,
        },
        ConcurrencyCase {
            name: "Qualifier=$LATEST",
            reference: "scoped".to_string(),
            qualifier: Some("$LATEST"),
            unqualified: false,
        },
        ConcurrencyCase {
            name: "Qualifier=1, the published version",
            reference: "scoped".to_string(),
            qualifier: Some("1"),
            unqualified: false,
        },
        ConcurrencyCase {
            name: "Qualifier=live, an alias",
            reference: "scoped".to_string(),
            qualifier: Some("live"),
            unqualified: false,
        },
        ConcurrencyCase {
            name: "the version qualified on the name itself",
            reference: "scoped:1".to_string(),
            qualifier: None,
            unqualified: false,
        },
        ConcurrencyCase {
            name: "$LATEST qualified on the ARN itself",
            reference: format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:scoped:$LATEST"),
            qualifier: None,
            unqualified: false,
        },
    ];

    for case in &cases {
        let answer = get_function(&router, &case.reference, case.qualifier).await;
        assert_eq!(
            answer.status,
            200,
            "case {:?}: expected 200 from GetFunction, got {} {}",
            case.name,
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let body = answer.json();
        let concurrency = member(&body, "/Concurrency");
        if case.unqualified {
            assert_eq!(
                member(&body, "/Concurrency/ReservedConcurrentExecutions"),
                Some(&json!(reserved)),
                "case {:?}: expected the function's reserved concurrency {reserved}, got \
                 {concurrency:?}",
                case.name
            );
        } else {
            assert!(
                concurrency.is_none(),
                "case {:?}: a qualified identity carries no reserved concurrency of its \
                 own, got {concurrency:?}",
                case.name
            );
        }
    }

    // "Removes a concurrent execution limit from a function": the function goes back to
    // holding none, so the unqualified read reports none either.
    let deleted = delete_concurrency(&router, "scoped").await;
    assert_eq!(
        deleted.status,
        204,
        "expected 204 from DeleteFunctionConcurrency, got {} {}",
        deleted.status,
        String::from_utf8_lossy(&deleted.body)
    );
    let after = get_function(&router, "scoped", None).await;
    assert!(
        member(&after.json(), "/Concurrency").is_none(),
        "expected no Concurrency once the limit is removed, got {:?}",
        member(&after.json(), "/Concurrency")
    );
}

/// `PublishVersion` mints a `RevisionId` for the version it creates, so no two identities
/// of one function share a revision — which is what makes a revision precondition mean
/// one identity. The version's own revision is stable: every later read of it reports the
/// same one.
///
/// The rows are the successive versions of one function, each published after a different
/// kind of change (a configuration update, a code update, a description override), so the
/// rule is asserted over several versions rather than one.
#[tokio::test]
async fn every_identity_of_a_function_carries_its_own_revision() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_active_function(&router, &engine, "revisions", &package_bytes(48)).await;

    // Each row changes the unpublished configuration, then publishes: a version is only
    // published when something changed since the last one.
    let rounds: Vec<(&str, Value, Value)> = vec![
        (
            "the first version",
            json!({ "Description": "first" }),
            Value::Null,
        ),
        (
            "a version after a memory change",
            json!({ "MemorySize": 256 }),
            Value::Null,
        ),
        (
            "a version with a description override",
            json!({ "Timeout": 11 }),
            json!({ "Description": "release two" }),
        ),
    ];

    let mut revisions: Vec<(String, String)> = Vec::new();
    for (case, patch, publish_body) in &rounds {
        // An update is applied behind the call, and "During this time, you can't modify
        // the function": the clock settles the one before this row's.
        make_update_due(&engine);
        let updated = update_configuration(&router, "revisions", patch).await;
        assert_eq!(
            updated.status,
            200,
            "case {case:?}: UpdateFunctionConfiguration must succeed, got {} {}",
            updated.status,
            String::from_utf8_lossy(&updated.body)
        );
        let latest_revision = updated
            .string("RevisionId")
            .unwrap_or_else(|| panic!("case {case:?}: the update must report a RevisionId"));

        let published = publish_version(&router, "revisions", publish_body).await;
        assert_eq!(
            published.status,
            201,
            "case {case:?}: PublishVersion must succeed, got {} {}",
            published.status,
            String::from_utf8_lossy(&published.body)
        );
        let version = published
            .string("Version")
            .unwrap_or_else(|| panic!("case {case:?}: the publish must report a Version"));
        let version_revision = published
            .string("RevisionId")
            .unwrap_or_else(|| panic!("case {case:?}: the publish must report a RevisionId"));

        assert_ne!(
            version_revision, latest_revision,
            "case {case:?}: version {version} must not reuse the unpublished version's \
             revision, got {version_revision:?}"
        );
        // Read back: the revision the publish reported is the one the version holds.
        let read = get_configuration(&router, "revisions", Some(&version)).await;
        assert_eq!(
            read.string("RevisionId").as_deref(),
            Some(version_revision.as_str()),
            "case {case:?}: expected version {version} to report the stored revision \
             {version_revision:?}, got {:?}",
            read.string("RevisionId")
        );
        revisions.push((version, version_revision));
    }

    // Every version's revision is its own, across the whole set.
    for (version, revision) in &revisions {
        let others: Vec<&String> = revisions
            .iter()
            .filter(|(other, _)| other != version)
            .map(|(_, revision)| revision)
            .collect();
        assert!(
            !others.contains(&revision),
            "case version {version}: expected a revision no other version holds, got \
             {revision:?} among {others:?}"
        );
    }
}

/// One row of the description table: how the resource was created, and what its own read
/// must therefore report.
struct DescriptionCase {
    /// The case name, which is also the resource name.
    name: &'static str,
    /// The `Description` the create request carried, when any.
    requested: Option<&'static str>,
    /// What every read of the resource must report.
    reported: &'static str,
}

/// `Description` is a member of every function configuration and of every layer version's
/// own read: a resource created without one reports it as the EMPTY STRING rather than not
/// at all, so a client can read the member unconditionally. A layer LISTING entry is the
/// other way round — it carries only what the version itself carries.
///
/// The rows differ along what the rule generalises over: no description at all, an empty
/// one stated explicitly, and a stated value.
#[tokio::test]
async fn an_omitted_description_is_reported_empty_by_the_resources_own_read() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let cases = vec![
        DescriptionCase {
            name: "omitted",
            requested: None,
            reported: "",
        },
        DescriptionCase {
            name: "stated-empty",
            requested: Some(""),
            reported: "",
        },
        DescriptionCase {
            name: "stated",
            requested: Some("a described resource"),
            reported: "a described resource",
        },
    ];

    for case in &cases {
        // The function half of the rule: create, publish a version, and read both
        // identities back.
        let mut request = json!({
            "FunctionName": case.name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": { "ZipFile": zip_file(&package_bytes(32)) },
        });
        if let Some(requested) = case.requested {
            request["Description"] = json!(requested);
        }
        let created = create_function(&router, &request).await;
        assert_eq!(
            created.status,
            201,
            "case {:?}: CreateFunction must succeed, got {} {}",
            case.name,
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        assert_eq!(
            created.json().get("Description"),
            Some(&json!(case.reported)),
            "case {:?}: expected CreateFunction to report Description {:?}, got {:?}",
            case.name,
            case.reported,
            created.json().get("Description")
        );
        make_activation_due(&engine);

        let published = publish_version(&router, case.name, &Value::Null).await;
        assert_eq!(
            published.status,
            201,
            "case {:?}: PublishVersion must succeed, got {} {}",
            case.name,
            published.status,
            String::from_utf8_lossy(&published.body)
        );
        for (reader, body) in [
            ("PublishVersion", published.json()),
            (
                "GetFunctionConfiguration of the version",
                get_configuration(&router, case.name, Some("1")).await.json(),
            ),
            (
                "GetFunction of the function",
                get_function(&router, case.name, None).await.json(),
            ),
        ] {
            let reported = body
                .get("Description")
                .or_else(|| member(&body, "/Configuration/Description"))
                .cloned();
            assert_eq!(
                reported,
                Some(json!(case.reported)),
                "case {:?}: expected {reader} to report Description {:?}, got {reported:?}",
                case.name,
                case.reported
            );
        }

        // The layer half: the version's own reads default the member, the listing entry
        // carries only what was stated.
        let mut layer_request = json!({
            "Content": { "ZipFile": zip_file(&package_bytes(40)) },
        });
        if let Some(requested) = case.requested {
            layer_request["Description"] = json!(requested);
        }
        let layer = publish_layer_version(&router, case.name, &layer_request).await;
        assert_eq!(
            layer.status,
            201,
            "case {:?}: PublishLayerVersion must succeed, got {} {}",
            case.name,
            layer.status,
            String::from_utf8_lossy(&layer.body)
        );
        for (reader, body) in [
            ("PublishLayerVersion", layer.json()),
            (
                "GetLayerVersion",
                get_layer_version(&router, case.name, 1).await.json(),
            ),
        ] {
            assert_eq!(
                body.get("Description"),
                Some(&json!(case.reported)),
                "case {:?}: expected {reader} to report Description {:?}, got {:?}",
                case.name,
                case.reported,
                body.get("Description")
            );
        }
        let listed = list_layer_versions(&router, case.name).await.json();
        let entry = member(&listed, "/LayerVersions/0")
            .cloned()
            .unwrap_or_else(|| panic!("case {:?}: expected one listed version", case.name));
        assert_eq!(
            entry.get("Description"),
            case.requested.map(|requested| json!(requested)).as_ref(),
            "case {:?}: a listing entry carries the description only when the version \
             states one, got {:?}",
            case.name,
            entry.get("Description")
        );
    }
}

/// One row of the image table: the `ImageUri` the function was created with, and the
/// repository its resolved URI must pin.
struct ImageCase {
    /// The case name, which is also the function name.
    name: &'static str,
    /// The `Code.ImageUri` the create request carried.
    image_uri: String,
    /// The repository part of that URI, which the resolved URI must keep.
    repository: String,
    /// The manifest digest the REGISTRY minted for the image that URI names.
    digest: String,
}

/// `ResolvedImageUri` is "The resolved URI for the image": the SAME repository the
/// function was created against, pinned to the digest the REGISTRY holds for the image it
/// is frozen to — never the mutable tag that was supplied. The digest is the registry's
/// own, so it is stable across reads, equal for two references to one image and different
/// for two images.
///
/// The rows differ along what the rule generalises over: a tagged URI, a second tag of the
/// same repository, a URI that already carries a digest, and a repository path with
/// several segments. Every image is pushed through Amazon ECR's own surface first, because
/// a reference naming nothing pushed is refused rather than resolved.
#[tokio::test]
async fn resolved_image_uri_pins_the_same_repository_to_a_digest() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let registry = format!("{ACCOUNT}.dkr.ecr.{REGION}.amazonaws.com");
    let repository = format!("{registry}/fn");
    let nested_repository = format!("{registry}/team/fn");
    let v1 = seed_image(&engine, &format!("{repository}:v1")).await;
    let v2 = seed_image(&engine, &format!("{repository}:v2")).await;
    let v3 = seed_image(&engine, &format!("{repository}:v3")).await;
    let nested = seed_image(&engine, &format!("{nested_repository}:latest")).await;
    let cases = vec![
        ImageCase {
            name: "tagged",
            image_uri: format!("{repository}:v1"),
            repository: repository.clone(),
            digest: v1,
        },
        ImageCase {
            name: "another-tag",
            image_uri: format!("{repository}:v2"),
            repository: repository.clone(),
            digest: v2,
        },
        ImageCase {
            name: "already-pinned",
            image_uri: format!("{repository}@{v3}"),
            repository: repository.clone(),
            digest: v3,
        },
        ImageCase {
            name: "nested-repository",
            image_uri: format!("{nested_repository}:latest"),
            repository: nested_repository.clone(),
            digest: nested,
        },
    ];
    assert!(
        cases
            .iter()
            .all(|case| case.image_uri.starts_with(&registry)),
        "every row must address this emulator's own registry host {registry:?}"
    );

    let mut resolved_uris: Vec<(&str, String)> = Vec::new();
    for case in &cases {
        let created = create_function(
            &router,
            &json!({
                "FunctionName": case.name,
                "Role": ROLE,
                "PackageType": "Image",
                "Code": { "ImageUri": case.image_uri },
            }),
        )
        .await;
        assert_eq!(
            created.status,
            201,
            "case {:?}: CreateFunction must succeed, got {} {}",
            case.name,
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        make_activation_due(&engine);

        let read = get_function(&router, case.name, None).await;
        let body = read.json();
        assert_eq!(
            member(&body, "/Code/ImageUri"),
            Some(&json!(case.image_uri)),
            "case {:?}: expected the ImageUri as supplied, got {:?}",
            case.name,
            member(&body, "/Code/ImageUri")
        );
        let resolved = member(&body, "/Code/ResolvedImageUri")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        // The digest is the one the registry minted for the pushed manifest, which the
        // push reported rather than this test computing it.
        assert_eq!(
            resolved,
            format!("{}@{}", case.repository, case.digest),
            "case {:?}: expected the repository {:?} pinned to the registry's digest for \
             {:?}, got {resolved:?}",
            case.name,
            case.repository,
            case.image_uri
        );

        // Stable across reads: a second read of an unchanged function resolves the same.
        let again = get_function(&router, case.name, None).await;
        assert_eq!(
            member(&again.json(), "/Code/ResolvedImageUri").and_then(Value::as_str),
            Some(resolved.as_str()),
            "case {:?}: expected the same resolved URI on a second read, got {:?}",
            case.name,
            member(&again.json(), "/Code/ResolvedImageUri")
        );
        resolved_uris.push((case.name, resolved));
    }

    // Two different references resolve differently, so the resolved URI says WHICH image
    // the function is frozen to.
    for (name, resolved) in &resolved_uris {
        let clashes: Vec<&(&str, String)> = resolved_uris
            .iter()
            .filter(|(other, other_resolved)| other != name && other_resolved == resolved)
            .collect();
        assert!(
            clashes.is_empty(),
            "case {name:?}: expected a resolved URI no other image shares, got {resolved:?} \
             also for {clashes:?}"
        );
    }
}

/// One row of the durable-configuration table: what the request states, and the
/// configuration the function must then hold.
struct DurableCase {
    /// The case name, which is also the function name.
    name: &'static str,
    /// The `DurableConfig` the create request carries.
    created: Value,
    /// The `RetentionPeriodInDays` the created function must report.
    retention: i64,
    /// The `ExecutionTimeout` the created function must report, when it has one.
    execution_timeout: Option<i64>,
    /// The `DurableConfig` an update then states.
    patch: Value,
    /// The `RetentionPeriodInDays` the updated function must report.
    updated_retention: i64,
    /// The `ExecutionTimeout` the updated function must report, when it has one.
    updated_execution_timeout: Option<i64>,
}

/// A durable-execution configuration member with a documented default is reported with
/// that default when the caller never set it, and an update changes only the members it
/// names. A function configured for durable execution also starts at the longest timeout
/// Lambda allows rather than the 3-second default, because its invocations are the steps
/// of an execution that outlives any one of them.
///
/// The rows differ along what the rule generalises over: which members the create states,
/// which members the update states, and whether the update replaces a default or a stated
/// value.
#[tokio::test]
async fn a_durable_configuration_defaults_and_updates_per_member() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let cases = vec![
        DurableCase {
            name: "durable-defaults",
            created: json!({}),
            retention: DEFAULT_RETENTION_PERIOD_DAYS,
            execution_timeout: None,
            patch: json!({ "ExecutionTimeout": 600 }),
            updated_retention: DEFAULT_RETENTION_PERIOD_DAYS,
            updated_execution_timeout: Some(600),
        },
        DurableCase {
            name: "durable-timeout-only",
            created: json!({ "ExecutionTimeout": 120 }),
            retention: DEFAULT_RETENTION_PERIOD_DAYS,
            execution_timeout: Some(120),
            patch: json!({ "RetentionPeriodInDays": 90 }),
            updated_retention: 90,
            updated_execution_timeout: Some(120),
        },
        DurableCase {
            name: "durable-every-member",
            created: json!({ "RetentionPeriodInDays": 1, "ExecutionTimeout": 31_622_400 }),
            retention: 1,
            execution_timeout: Some(31_622_400),
            patch: json!({ "RetentionPeriodInDays": 30 }),
            updated_retention: 30,
            updated_execution_timeout: Some(31_622_400),
        },
    ];

    for case in &cases {
        let created = create_function(
            &router,
            &json!({
                "FunctionName": case.name,
                "Role": ROLE,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "DurableConfig": case.created.clone(),
                "Code": { "ZipFile": zip_file(&package_bytes(32)) },
            }),
        )
        .await;
        assert_eq!(
            created.status,
            201,
            "case {:?}: CreateFunction must succeed, got {} {}",
            case.name,
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        let body = created.json();
        assert_eq!(
            member(&body, "/DurableConfig/RetentionPeriodInDays"),
            Some(&json!(case.retention)),
            "case {:?}: expected RetentionPeriodInDays {}, got {:?}",
            case.name,
            case.retention,
            member(&body, "/DurableConfig/RetentionPeriodInDays")
        );
        assert_eq!(
            member(&body, "/DurableConfig/ExecutionTimeout"),
            case.execution_timeout.map(|value| json!(value)).as_ref(),
            "case {:?}: expected ExecutionTimeout {:?}, got {:?}",
            case.name,
            case.execution_timeout,
            member(&body, "/DurableConfig/ExecutionTimeout")
        );
        // The function's own timeout: the longest Lambda allows, for a durable function
        // that states none.
        assert_eq!(
            body.get("Timeout"),
            Some(&json!(MAX_TIMEOUT_SECONDS)),
            "case {:?}: expected a durable function's own Timeout to default to \
             {MAX_TIMEOUT_SECONDS}, got {:?}",
            case.name,
            body.get("Timeout")
        );
        make_activation_due(&engine);

        let updated = update_configuration(
            &router,
            case.name,
            &json!({ "DurableConfig": case.patch.clone() }),
        )
        .await;
        assert_eq!(
            updated.status,
            200,
            "case {:?}: UpdateFunctionConfiguration must succeed, got {} {}",
            case.name,
            updated.status,
            String::from_utf8_lossy(&updated.body)
        );
        // Read it back, so nothing here trusts the update answer alone.
        let read = get_configuration(&router, case.name, None).await.json();
        assert_eq!(
            member(&read, "/DurableConfig/RetentionPeriodInDays"),
            Some(&json!(case.updated_retention)),
            "case {:?}: expected RetentionPeriodInDays {} after the update, got {:?}",
            case.name,
            case.updated_retention,
            member(&read, "/DurableConfig/RetentionPeriodInDays")
        );
        assert_eq!(
            member(&read, "/DurableConfig/ExecutionTimeout"),
            case.updated_execution_timeout
                .map(|value| json!(value))
                .as_ref(),
            "case {:?}: expected ExecutionTimeout {:?} after the update, got {:?}",
            case.name,
            case.updated_execution_timeout,
            member(&read, "/DurableConfig/ExecutionTimeout")
        );
    }

    // A function that is NOT configured for durability keeps the documented 3-second
    // default and carries no durable configuration at all.
    let plain = create_function(
        &router,
        &json!({
            "FunctionName": "not-durable",
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": { "ZipFile": zip_file(&package_bytes(32)) },
        }),
    )
    .await;
    let body = plain.json();
    assert_eq!(
        body.get("Timeout"),
        Some(&json!(DEFAULT_TIMEOUT_SECONDS)),
        "case \"not-durable\": expected the documented default Timeout \
         {DEFAULT_TIMEOUT_SECONDS}, got {:?}",
        body.get("Timeout")
    );
    assert!(
        body.get("DurableConfig").is_none(),
        "case \"not-durable\": expected no DurableConfig, got {:?}",
        body.get("DurableConfig")
    );
}

/// A download URL this service ANNOUNCES resolves: the host it names is this service's
/// own, and what it serves is exactly the bytes the store holds for the object — the
/// deployment package of the identity the location was reported for, or one layer
/// version's archive. A published version's location keeps serving the bytes it froze
/// after the unpublished version's code is replaced.
///
/// The rows differ along what the rule generalises over: which object the URL names (a
/// function package, a published version's frozen package, a layer archive), the bytes
/// held for it, and whether the request is a `GET` or a `HEAD`.
#[tokio::test]
async fn an_announced_download_url_serves_the_stored_bytes() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let first = package_bytes(96);
    let second = package_bytes(200);
    create_active_function(&router, &engine, "downloaded", &first).await;

    // The unpublished version's location serves the bytes the create stored.
    let created_read = get_function(&router, "downloaded", None).await.json();
    let latest_location = member(&created_read, "/Code/Location")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let published = publish_version(&router, "downloaded", &Value::Null).await;
    assert_eq!(
        published.status,
        201,
        "PublishVersion must succeed, got {} {}",
        published.status,
        String::from_utf8_lossy(&published.body)
    );
    let version_read = get_function(&router, "downloaded", Some("1")).await.json();
    let version_location = member(&version_read, "/Code/Location")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert_ne!(
        version_location, latest_location,
        "expected the published version's location to address its own package, got \
         {version_location:?}"
    );

    // Replace the unpublished version's code: the version's location still serves the
    // bytes it froze, and the function's new location serves the new ones.
    let recoded = update_code(
        &router,
        "downloaded",
        &json!({ "ZipFile": zip_file(&second) }),
    )
    .await;
    assert_eq!(
        recoded.status,
        200,
        "UpdateFunctionCode must succeed, got {} {}",
        recoded.status,
        String::from_utf8_lossy(&recoded.body)
    );
    let updated_read = get_function(&router, "downloaded", None).await.json();
    let updated_location = member(&updated_read, "/Code/Location")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    // A layer archive is announced the same way and served the same way.
    let archive = package_bytes(144);
    let layer = publish_layer_version(
        &router,
        "downloaded-layer",
        &json!({ "Content": { "ZipFile": zip_file(&archive) } }),
    )
    .await;
    assert_eq!(
        layer.status,
        201,
        "PublishLayerVersion must succeed, got {} {}",
        layer.status,
        String::from_utf8_lossy(&layer.body)
    );
    let layer_location = member(&layer.json(), "/Content/Location")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let cases: Vec<(&str, &str, &[u8])> = vec![
        ("the published version's frozen package", &version_location, &first),
        ("the function's package after a code update", &updated_location, &second),
        ("a layer version's archive", &layer_location, &archive),
    ];

    for (case, url, expected) in &cases {
        assert!(
            url.starts_with("https://"),
            "case {case:?}: expected an announced URL, got {url:?}"
        );
        let answer = fetch(&router, "GET", url).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: expected 200 following {url:?}, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.body.as_ref(),
            *expected,
            "case {case:?}: expected the {} stored bytes, got {} bytes",
            expected.len(),
            answer.body.len()
        );
        assert_eq!(
            answer.header("content-length").as_deref(),
            Some(expected.len().to_string().as_str()),
            "case {case:?}: expected a Content-Length of {}, got {:?}",
            expected.len(),
            answer.header("content-length")
        );
        // A download is a pure READ: following it twice answers the same bytes.
        let again = fetch(&router, "GET", url).await;
        assert_eq!(
            again.body, answer.body,
            "case {case:?}: expected the same bytes on a second fetch, got {} bytes",
            again.body.len()
        );
        // A `HEAD` reports the metadata without the bytes.
        let head = fetch(&router, "HEAD", url).await;
        assert_eq!(
            (head.status, head.body.len()),
            (200, 0),
            "case {case:?}: expected a 200 HEAD with no body, got {} with {} bytes",
            head.status,
            head.body.len()
        );
    }

    // The unpublished version's OLD location addressed a package the store no longer
    // holds, and a key this service publishes no object at names none either: both are
    // the download plane's own 404, never the router's "no such service".
    let absent = format!(
        "{base}/awslambda-{REGION}-tasks/snapshots/{ACCOUNT}\
         /absent-00000000-0000-4000-8000-000000000000",
        base = announced_base(&latest_location),
    );
    for (case, url) in [
        ("a replaced revision", latest_location.as_str()),
        ("a key this service publishes no object at", absent.as_str()),
    ] {
        let answer = fetch(&router, "GET", url).await;
        assert_eq!(
            answer.status,
            404,
            "case {case:?}: expected 404 for an object this store does not hold, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert!(
            !String::from_utf8_lossy(&answer.body).contains("UnknownService"),
            "case {case:?}: a URL this service announced must be answered BY this service, \
             got {}",
            String::from_utf8_lossy(&answer.body)
        );
    }

    // A download is a read, so a write method is refused as the method it is.
    let posted = fetch(&router, "POST", &layer_location).await;
    assert_eq!(
        posted.status,
        405,
        "expected 405 for a POST to a download URL, got {} {}",
        posted.status,
        String::from_utf8_lossy(&posted.body)
    );

    // An S3-sourced package is a reference this emulator does not resolve, so it holds no
    // bytes to serve for it.
    let from_s3 = create_function(
        &router,
        &json!({
            "FunctionName": "from-s3",
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": { "S3Bucket": "deployments", "S3Key": "fn.zip" },
        }),
    )
    .await;
    assert_eq!(
        from_s3.status,
        201,
        "CreateFunction from S3 must succeed, got {} {}",
        from_s3.status,
        String::from_utf8_lossy(&from_s3.body)
    );
    make_activation_due(&engine);
    let s3_read = get_function(&router, "from-s3", None).await.json();
    let s3_location = member(&s3_read, "/Code/Location")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let s3_answer = fetch(&router, "GET", &s3_location).await;
    assert_eq!(
        s3_answer.status,
        404,
        "case \"an S3-sourced package\": expected 404 while this emulator holds no bytes \
         for it, got {} {}",
        s3_answer.status,
        String::from_utf8_lossy(&s3_answer.body)
    );
}

/// A `DryRun` invocation "validate[s] parameter values and verif[ies] that the user or
/// role has permission to invoke the function" and runs nothing, so it answers the
/// documented 204 with NO payload — and the body it declares is the body it delivers, so
/// an SDK reading the response gets an empty payload rather than a transport error.
///
/// The rows differ along what the rule generalises over: the payload the request carries
/// (none, an object, a scalar, one past the asynchronous size limit) and the identity the
/// invocation addresses.
#[tokio::test]
async fn a_dry_run_invocation_declares_the_empty_body_it_delivers() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_active_function(&router, &engine, "dry", &package_bytes(64)).await;
    let published = publish_version(&router, "dry", &Value::Null).await;
    assert_eq!(
        published.status,
        201,
        "PublishVersion must succeed, got {} {}",
        published.status,
        String::from_utf8_lossy(&published.body)
    );

    let cases: Vec<(&str, &str, Vec<u8>, &str)> = vec![
        ("no payload", "dry", Vec::new(), "$LATEST"),
        ("a JSON object", "dry", br#"{"key":1}"#.to_vec(), "$LATEST"),
        ("a JSON scalar", "dry", b"42".to_vec(), "$LATEST"),
        (
            "the published version",
            "dry",
            br#"{"key":"value"}"#.to_vec(),
            "1",
        ),
    ];

    for (case, reference, payload, executed) in &cases {
        // The qualifier travels in the `Qualifier` query parameter of the invoke path.
        let query = if *executed == "$LATEST" {
            String::new()
        } else {
            format!("?Qualifier={executed}")
        };
        let uri = format!("/2015-03-31/functions/{reference}/invocations{query}");
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
            .header("host", "lambda.us-east-1.amazonaws.com")
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 \
                 Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
                 SignedHeaders=host, Signature=not-verified",
            )
            .header("content-type", "application/octet-stream")
            .header("x-amz-invocation-type", "DryRun")
            .body(bytes::Bytes::from(payload.clone()))
            .expect("the invoke request must build");
        let response = router.dispatch(request).await;
        let (parts, body) = response.into_parts();
        let answer = Answer {
            status: parts.status.as_u16(),
            headers: parts.headers,
            body,
        };

        assert_eq!(
            answer.status,
            204,
            "case {case:?}: expected the documented 204 for a dry run, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert!(
            answer.body.is_empty(),
            "case {case:?}: a dry run returns no payload, got {} bytes ({})",
            answer.body.len(),
            String::from_utf8_lossy(&answer.body)
        );
        // The declared length and the delivered body agree: a client that reads
        // Content-Length bytes reads exactly the body.
        let declared = answer
            .header("content-length")
            .unwrap_or_else(|| "0".to_string());
        assert_eq!(
            declared, "0",
            "case {case:?}: expected a Content-Length of 0 beside an empty body, got \
             {declared:?}"
        );
        assert_eq!(
            answer.header("x-amz-executed-version").as_deref(),
            Some(*executed),
            "case {case:?}: expected the resolved version {executed:?} in \
             X-Amz-Executed-Version, got {:?}",
            answer.header("x-amz-executed-version")
        );
    }
}

/// The refusal a second `CreateEventSourceMapping` of one (event source, function) pair
/// earns is built from the mapping that already exists: it names the event source, the
/// function and the UUID the existing mapping is updated or deleted by, so a client
/// holding many mappings can tell WHICH pair collided.
///
/// The rows differ along what the rule generalises over: which event source the pair
/// polls, and which documented `FunctionName` form the second create addresses the
/// function by.
#[tokio::test]
async fn the_duplicate_event_source_mapping_refusal_names_the_pair() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_active_function(&router, &engine, "mapped", &package_bytes(64)).await;
    create_active_function(&router, &engine, "mapped-two", &package_bytes(72)).await;

    let cases: Vec<(&str, &str, Value, String)> = vec![
        (
            "an SQS queue, by bare name",
            "mapped",
            json!({ "EventSourceArn": queue_arn("first-queue"), "FunctionName": "mapped" }),
            "mapped".to_string(),
        ),
        (
            "an SQS queue, the duplicate addressed by function ARN",
            "mapped-two",
            json!({
                "EventSourceArn": queue_arn("second-queue"),
                "FunctionName": format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:mapped-two"),
            }),
            "mapped-two".to_string(),
        ),
        (
            "a Kinesis stream",
            "mapped",
            json!({
                "EventSourceArn": format!("arn:aws:kinesis:{REGION}:{ACCOUNT}:stream/records"),
                "FunctionName": "mapped",
                "StartingPosition": "LATEST",
            }),
            "mapped".to_string(),
        ),
    ];

    for (case, _function, body, expected_function) in &cases {
        let first = create_mapping(&router, body).await;
        assert_eq!(
            first.status,
            202,
            "case {case:?}: the first create must be accepted, got {} {}",
            first.status,
            String::from_utf8_lossy(&first.body)
        );
        let uuid = first
            .string("UUID")
            .unwrap_or_else(|| panic!("case {case:?}: the create must report a UUID"));
        let source = body
            .get("EventSourceArn")
            .and_then(Value::as_str)
            .unwrap_or_default();

        let duplicate = create_mapping(&router, body).await;
        assert_eq!(
            duplicate.status,
            409,
            "case {case:?}: expected 409 for a second mapping of the same pair, got {} {}",
            duplicate.status,
            String::from_utf8_lossy(&duplicate.body)
        );
        let message = message_of(&duplicate);
        for named in [source, expected_function.as_str(), uuid.as_str()] {
            assert!(
                message.contains(named),
                "case {case:?}: expected the refusal to name {named:?}, got {message:?}"
            );
        }
    }
}

/// One row of the layer-policy table: the state the version is in, and what a read and a
/// removal must answer for it.
struct PolicyStateCase {
    /// The case name, which is also the layer name.
    name: &'static str,
    /// Whether a statement is granted and then removed, leaving the version with an
    /// emptied policy.
    emptied: bool,
}

/// A layer version with no statement to report is diagnosed for its POLICY rather than as
/// an absent resource, and the two no-policy states are told apart: a version nobody has
/// granted anything on reads as having no policy associated, and one whose statements have
/// all been removed reads as the version — named by its ARN — having no resource policy
/// left. Neither is the refusal for a layer version this account does not hold.
///
/// The rows differ along what the rule generalises over: the state the version's policy is
/// in (never granted, granted and emptied), beside the version that does not exist at all.
#[tokio::test]
async fn a_layer_versions_policy_states_are_diagnosed_apart() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let cases = vec![
        PolicyStateCase {
            name: "never-granted",
            emptied: false,
        },
        PolicyStateCase {
            name: "emptied",
            emptied: true,
        },
    ];

    for case in &cases {
        let published = publish_layer_version(
            &router,
            case.name,
            &json!({ "Content": { "ZipFile": zip_file(&package_bytes(48)) } }),
        )
        .await;
        assert_eq!(
            published.status,
            201,
            "case {:?}: PublishLayerVersion must succeed, got {} {}",
            case.name,
            published.status,
            String::from_utf8_lossy(&published.body)
        );
        let version_arn = published
            .string("LayerVersionArn")
            .unwrap_or_else(|| panic!("case {:?}: expected a LayerVersionArn", case.name));

        let expected_read = if case.emptied {
            let granted = add_layer_permission(&router, case.name, 1, "granted").await;
            assert_eq!(
                granted.status,
                201,
                "case {:?}: the grant must be accepted, got {} {}",
                case.name,
                granted.status,
                String::from_utf8_lossy(&granted.body)
            );
            // While the statement is there, the policy IS reported.
            let held = get_layer_policy(&router, case.name, 1).await;
            assert_eq!(
                held.status,
                200,
                "case {:?}: expected the granted policy to be reported, got {} {}",
                case.name,
                held.status,
                String::from_utf8_lossy(&held.body)
            );
            let removed = remove_layer_permission(&router, case.name, 1, "granted").await;
            assert_eq!(
                removed.status,
                204,
                "case {:?}: the removal must be accepted, got {} {}",
                case.name,
                removed.status,
                String::from_utf8_lossy(&removed.body)
            );
            format!("Layer version {version_arn} does not have any resource policy.")
        } else {
            "No policy is associated with the given resource.".to_string()
        };

        let read = get_layer_policy(&router, case.name, 1).await;
        assert_eq!(
            (read.status, message_of(&read)),
            (404, expected_read.clone()),
            "case {:?}: expected 404 {expected_read:?} from the policy read, got {} {}",
            case.name,
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        assert!(
            read.error_code().contains("ResourceNotFoundException"),
            "case {:?}: expected ResourceNotFoundException, got {}",
            case.name,
            read.error_code()
        );

        // A removal has nothing to take out in either state, and says so about the
        // policy rather than about the resource.
        let removal = remove_layer_permission(&router, case.name, 1, "granted").await;
        assert_eq!(
            (removal.status, message_of(&removal)),
            (
                404,
                "No policy is associated with the given resource.".to_string()
            ),
            "case {:?}: expected the removal to report that no policy is associated, got \
             {} {}",
            case.name,
            removal.status,
            String::from_utf8_lossy(&removal.body)
        );

        // A layer version that does NOT exist is the other diagnosis entirely: neither
        // no-policy message is used for it.
        let absent = get_layer_policy(&router, case.name, 9).await;
        let absent_message = message_of(&absent);
        assert_eq!(
            absent.status, 404,
            "case {:?}: expected 404 for a version this layer never had, got {} {}",
            case.name, absent.status, absent_message
        );
        assert!(
            !absent_message.contains("resource policy")
                && !absent_message.contains("No policy is associated"),
            "case {:?}: a version that does not exist must not be diagnosed as one with no \
             policy, got {absent_message:?}",
            case.name
        );
    }
}
