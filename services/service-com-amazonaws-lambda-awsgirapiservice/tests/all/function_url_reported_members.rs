// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHAT a function-URL answer reports, member by member: the
//! `InvokeMode` a write answers against the one a read reports, the form every ISO-8601
//! instant this service renders, the `NextMarker` a page hands back, and the refusal a
//! qualifier outside its member's modelled pattern gets.
//!
//! Three rules, each asserted over a set of inputs rather than one request:
//!
//! 1. An optional member is reported as CONFIGURED by the call that configures it: a
//!    write (`CreateFunctionUrlConfig`, `UpdateFunctionUrlConfig`) answers the members it
//!    persisted, so an `InvokeMode` no request ever named is absent from its answer,
//!    while a READ of the same configuration (`GetFunctionUrlConfig`,
//!    `ListFunctionUrlConfigs`) reports the mode the URL SERVES IN — the documented
//!    `BUFFERED` default while none is configured.
//! 2. Every ISO-8601 instant this service reports carries a NUMERIC UTC offset rather
//!    than the `Z` designator, which is the form the model's own instant example spells:
//!    `LayerVersionsListItem$CreatedDate` is documented "in ISO 8601 format. For example,
//!    `2018-11-27T15:10:45.123+0000`"
//!    (`smithy_query(query="doc", shape="LayerVersionsListItem", member="CreatedDate")`),
//!    and every other instant member is documented by the same phrase ("in ISO-8601
//!    format (YYYY-MM-DDThh:mm:ss.sTZD)"). The rows span the operations that mint one.
//! 3. `NextMarker` is "The pagination token that's included if more results are
//!    available": a token the listing MINTS for the position a truncated page ended on,
//!    so it is none of the identifiers this API addresses a resource by, it is present
//!    only while entries remain, and a value the listing did not mint names no position.
//!
//! Every row drives real HTTP requests through the router and reads the configuration
//! back, so a member answered but never persisted fails here.

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// Where the function-URL operations are bound.
const URL_API_VERSION: &str = "/2021-10-31/functions";

/// Where `PublishLayerVersion` is bound.
const LAYERS_URI: &str = "/2018-10-31/layers";

/// Where `CreateCodeSigningConfig` is bound.
const CODE_SIGNING_CONFIGS_URI: &str = "/2020-04-22/code-signing-configs";

/// Where `CreateCapacityProvider` is bound.
const CAPACITY_PROVIDERS_URI: &str = "/2025-11-30/capacity-providers";

/// The router picks a service by the SigV4 credential scope's service field, so every
/// request carries one naming `lambda` in `us-east-1`. Nothing in it is verified: this
/// harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The region the scope above names.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The unpublished version label, as the model's `Version` pattern
/// (`^(\$LATEST|[0-9]+)$`) spells it.
const VERSION_LATEST: &str = "$LATEST";

/// The `InvokeMode` the member's documentation names the default behaviour: "BUFFERED –
/// This is the default option."
const DEFAULT_INVOKE_MODE: &str = "BUFFERED";

/// The other value of the `InvokeMode` enum
/// (`smithy_query(query="shape", shape="InvokeMode")`: `BUFFERED | RESPONSE_STREAM`).
const STREAMING_INVOKE_MODE: &str = "RESPONSE_STREAM";

/// What a dispatched request answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Answer {
    /// The body as JSON, or `Value::Null` for an empty body.
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// A `String` member of the JSON body.
    fn string(&self, member: &str) -> Option<String> {
        self.json()
            .get(member)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// A member of the JSON body as it stands, `Null` when the answer omits it.
    fn member(&self, member: &str) -> Value {
        self.json().get(member).cloned().unwrap_or(Value::Null)
    }

    /// The wire error code (`__type`, or the `x-amzn-errortype` header restJson1 resolves
    /// an error shape from).
    fn error_code(&self) -> String {
        self.string("__type")
            .or_else(|| {
                self.headers
                    .get("x-amzn-errortype")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| String::from_utf8_lossy(&self.body).to_string())
    }

    /// The refusal's message, however the body spells the member.
    fn message(&self) -> String {
        self.string("message")
            .or_else(|| self.string("Message"))
            .unwrap_or_default()
    }

    /// The body text, for a failure message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// One service mounted on its own engine, reached the way the production router reaches
/// it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one JSON request.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// A deployment package: bytes this emulator stores as the function's archive.
fn package() -> String {
    let bytes: Vec<u8> = (0..64u16)
        .map(|byte| u8::try_from(byte % 251).unwrap_or(0))
        .collect();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Create a function, publishing version 1 so an alias has something to point at.
async fn create_function(router: &MultiServiceRouter, name: &str) -> Answer {
    let answer = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Publish": true,
            "Code": { "ZipFile": package() },
        }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "CreateFunction {name:?} must succeed, got {} {}",
        answer.status,
        answer.text()
    );
    answer
}

/// Create an alias of `function` pointing at version 1.
async fn create_alias(router: &MultiServiceRouter, function: &str, alias: &str) {
    let answer = call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{function}/aliases"),
        &json!({ "Name": alias, "FunctionVersion": "1" }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "CreateAlias {alias:?} on {function:?} must succeed, got {} {}",
        answer.status,
        answer.text()
    );
}

/// The URI of the function-URL resource of `reference`, at `qualifier` when one is named.
fn url_uri(reference: &str, qualifier: Option<&str>) -> String {
    let query = qualifier.map_or_else(String::new, |value| format!("?Qualifier={value}"));
    format!("{URL_API_VERSION}/{reference}/url{query}")
}

/// `CreateFunctionUrlConfig` on `reference`, at `qualifier` when one is named.
async fn create_url_config(
    router: &MultiServiceRouter,
    reference: &str,
    qualifier: Option<&str>,
    body: &Value,
) -> Answer {
    call(router, "POST", &url_uri(reference, qualifier), body).await
}

/// `UpdateFunctionUrlConfig` on `reference`, at `qualifier` when one is named.
async fn update_url_config(
    router: &MultiServiceRouter,
    reference: &str,
    qualifier: Option<&str>,
    body: &Value,
) -> Answer {
    call(router, "PUT", &url_uri(reference, qualifier), body).await
}

/// `GetFunctionUrlConfig` on `reference`, at `qualifier` when one is named.
async fn get_url_config(
    router: &MultiServiceRouter,
    reference: &str,
    qualifier: Option<&str>,
) -> Answer {
    call(
        router,
        "GET",
        &url_uri(reference, qualifier),
        &Value::Null,
    )
    .await
}

/// `DeleteFunctionUrlConfig` on `reference`, at `qualifier` when one is named.
async fn delete_url_config(
    router: &MultiServiceRouter,
    reference: &str,
    qualifier: Option<&str>,
) -> Answer {
    call(
        router,
        "DELETE",
        &url_uri(reference, qualifier),
        &Value::Null,
    )
    .await
}

/// `ListFunctionUrlConfigs` on `function`, with the given query string.
async fn list_url_configs(router: &MultiServiceRouter, function: &str, query: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("{URL_API_VERSION}/{function}/urls{query}"),
        &Value::Null,
    )
    .await
}

/// The entry of a listing whose `FunctionArn` ends in `qualifier`'s suffix, or `Null`.
fn listed_entry(answer: &Answer, arn: &str) -> Value {
    answer.json()["FunctionUrlConfigs"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|entry| entry["FunctionArn"] == json!(arn))
        .cloned()
        .unwrap_or(Value::Null)
}

/// The ARN of `function` at `qualifier`: the unqualified one for the unpublished version.
fn qualified_arn(function: &str, qualifier: &str) -> String {
    let base = format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{function}");
    if qualifier == VERSION_LATEST {
        base
    } else {
        format!("{base}:{qualifier}")
    }
}

/// One row of the invoke-mode table: what the create configured, what an update then
/// configures, and which qualifier of the function carries the configuration.
struct InvokeModeCase {
    name: &'static str,
    function: &'static str,
    /// The qualifier the configuration sits on; `None` is the unpublished version.
    qualifier: Option<&'static str>,
    create: Value,
    /// The update to apply, when the row applies one.
    update: Option<Value>,
    /// The `InvokeMode` the create must answer.
    created: Value,
    /// The `InvokeMode` the update must answer, for a row that applies one.
    updated: Value,
    /// The `InvokeMode` a READ must report: the mode the URL serves in.
    served: &'static str,
}

/// The rows. They differ along what the rule generalises over: whether the member is
/// configured at all, which of the two modelled values it is configured to, whether it is
/// configured by the create or by an update, whether an update that states OTHER members
/// leaves it alone, and which qualifier of the function the configuration sits on.
fn invoke_mode_cases() -> Vec<InvokeModeCase> {
    vec![
        InvokeModeCase {
            name: "never configured, on the unpublished version",
            function: "im-unset",
            qualifier: None,
            create: json!({ "AuthType": "NONE" }),
            update: None,
            created: Value::Null,
            updated: Value::Null,
            served: DEFAULT_INVOKE_MODE,
        },
        InvokeModeCase {
            name: "configured to the default value by the create",
            function: "im-buffered",
            qualifier: None,
            create: json!({ "AuthType": "NONE", "InvokeMode": DEFAULT_INVOKE_MODE }),
            update: None,
            created: json!(DEFAULT_INVOKE_MODE),
            updated: Value::Null,
            served: DEFAULT_INVOKE_MODE,
        },
        InvokeModeCase {
            name: "configured to the streaming value by the create, on an alias",
            function: "im-streaming",
            qualifier: Some("live"),
            create: json!({ "AuthType": "AWS_IAM", "InvokeMode": STREAMING_INVOKE_MODE }),
            update: None,
            created: json!(STREAMING_INVOKE_MODE),
            updated: Value::Null,
            served: STREAMING_INVOKE_MODE,
        },
        InvokeModeCase {
            name: "unset by the create and configured by the update",
            function: "im-set-later",
            qualifier: None,
            create: json!({ "AuthType": "NONE" }),
            update: Some(json!({ "InvokeMode": STREAMING_INVOKE_MODE })),
            created: Value::Null,
            updated: json!(STREAMING_INVOKE_MODE),
            served: STREAMING_INVOKE_MODE,
        },
        InvokeModeCase {
            name: "unset throughout, with an update that states another member",
            function: "im-still-unset",
            qualifier: Some("staging"),
            create: json!({ "AuthType": "NONE" }),
            update: Some(json!({ "AuthType": "AWS_IAM" })),
            created: Value::Null,
            updated: Value::Null,
            served: DEFAULT_INVOKE_MODE,
        },
        InvokeModeCase {
            name: "configured by the create and kept by an update that omits it",
            function: "im-kept",
            qualifier: None,
            create: json!({ "AuthType": "NONE", "InvokeMode": STREAMING_INVOKE_MODE }),
            update: Some(json!({ "Cors": { "AllowOrigins": ["https://www.example.com"] } })),
            created: json!(STREAMING_INVOKE_MODE),
            updated: json!(STREAMING_INVOKE_MODE),
            served: STREAMING_INVOKE_MODE,
        },
        InvokeModeCase {
            name: "configured by the create and changed back by an update",
            function: "im-changed",
            qualifier: None,
            create: json!({ "AuthType": "NONE", "InvokeMode": STREAMING_INVOKE_MODE }),
            update: Some(json!({ "InvokeMode": DEFAULT_INVOKE_MODE })),
            created: json!(STREAMING_INVOKE_MODE),
            updated: json!(DEFAULT_INVOKE_MODE),
            served: DEFAULT_INVOKE_MODE,
        },
    ]
}

/// A write answers the `InvokeMode` it persisted — absent while no caller configured one
/// — and a read reports the mode the URL serves in.
#[tokio::test]
async fn an_unconfigured_invoke_mode_is_absent_from_a_write_and_default_on_a_read() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    for case in invoke_mode_cases() {
        let label = case.name;
        create_function(&router, case.function).await;
        for alias in ["live", "staging"] {
            create_alias(&router, case.function, alias).await;
        }

        let created = create_url_config(&router, case.function, case.qualifier, &case.create).await;
        assert_eq!(
            created.status,
            201,
            "case {label:?}: expected 201 from CreateFunctionUrlConfig, got {} {}",
            created.status,
            created.text()
        );
        assert_eq!(
            created.member("InvokeMode"),
            case.created,
            "case {label:?}: the create must answer the InvokeMode it configured ({:?}), got {:?}",
            case.created,
            created.member("InvokeMode")
        );

        if let Some(update) = &case.update {
            let updated =
                update_url_config(&router, case.function, case.qualifier, update).await;
            assert_eq!(
                updated.status,
                200,
                "case {label:?}: expected 200 from UpdateFunctionUrlConfig, got {} {}",
                updated.status,
                updated.text()
            );
            assert_eq!(
                updated.member("InvokeMode"),
                case.updated,
                "case {label:?}: the update must answer the configured InvokeMode ({:?}), \
                 got {:?}",
                case.updated,
                updated.member("InvokeMode")
            );
        }

        // Both readers report the mode the URL serves in, and they agree.
        let read = get_url_config(&router, case.function, case.qualifier).await;
        assert_eq!(
            (read.status, read.member("InvokeMode")),
            (200, json!(case.served)),
            "case {label:?}: GetFunctionUrlConfig must report the mode the URL serves in \
             ({:?}), got {} {}",
            case.served,
            read.status,
            read.text()
        );
        let arn = qualified_arn(case.function, case.qualifier.unwrap_or(VERSION_LATEST));
        let listed = list_url_configs(&router, case.function, "").await;
        assert_eq!(
            listed_entry(&listed, &arn)["InvokeMode"],
            json!(case.served),
            "case {label:?}: the listed entry must report the same mode as the read ({:?}), \
             got {}",
            case.served,
            listed.text()
        );
    }
}

/// Assert `value` is an instant of the form this service renders: a date, a `T`, a time, a
/// three-digit millisecond field and the NUMERIC UTC offset — the form the model's own
/// instant example spells (`2018-11-27T15:10:45.123+0000`).
fn assert_offset_instant(case: &str, member: &str, value: &Value) {
    let rendered = value.as_str().unwrap_or_else(|| {
        panic!("case {case:?}: expected the string member {member} in {value:?}")
    });
    let template = "2018-11-27T15:10:45.123+0000";
    assert_eq!(
        rendered.len(),
        template.len(),
        "case {case:?}: {member} must be rendered like {template:?}, got {rendered:?}"
    );
    for (index, separator) in template.char_indices() {
        let actual = rendered[index..].chars().next().unwrap_or('\0');
        if separator.is_ascii_digit() {
            assert!(
                actual.is_ascii_digit(),
                "case {case:?}: {member} must carry a digit where {template:?} does \
                 (position {index}), got {rendered:?}"
            );
        } else {
            assert_eq!(
                actual, separator,
                "case {case:?}: {member} must carry {separator:?} at position {index}, got \
                 {rendered:?}"
            );
        }
    }
    assert!(
        !rendered.ends_with('Z'),
        "case {case:?}: {member} must carry the numeric UTC offset rather than the `Z` \
         designator, got {rendered:?}"
    );
}

/// Every instant this service reports is rendered in that one form, whichever operation
/// mints it.
#[tokio::test]
async fn every_reported_instant_carries_the_numeric_utc_offset() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // A function: `FunctionConfiguration$LastModified`.
    let function = create_function(&router, "instant-probe").await;
    assert_offset_instant("CreateFunction", "LastModified", &function.member("LastModified"));
    let configuration = call(
        &router,
        "GET",
        &format!("{FUNCTIONS_URI}/instant-probe/configuration"),
        &Value::Null,
    )
    .await;
    assert_offset_instant(
        "GetFunctionConfiguration",
        "LastModified",
        &configuration.member("LastModified"),
    );

    // A function URL: `CreationTime` on the create, both instants on the read, the
    // listing and the update.
    create_alias(&router, "instant-probe", "live").await;
    let created = create_url_config(
        &router,
        "instant-probe",
        None,
        &json!({ "AuthType": "NONE" }),
    )
    .await;
    assert_offset_instant(
        "CreateFunctionUrlConfig",
        "CreationTime",
        &created.member("CreationTime"),
    );
    let read = get_url_config(&router, "instant-probe", None).await;
    for member in ["CreationTime", "LastModifiedTime"] {
        assert_offset_instant("GetFunctionUrlConfig", member, &read.member(member));
    }
    let updated = update_url_config(
        &router,
        "instant-probe",
        None,
        &json!({ "AuthType": "AWS_IAM" }),
    )
    .await;
    for member in ["CreationTime", "LastModifiedTime"] {
        assert_offset_instant("UpdateFunctionUrlConfig", member, &updated.member(member));
    }
    let listed = list_url_configs(&router, "instant-probe", "").await;
    let entry = listed_entry(&listed, &qualified_arn("instant-probe", VERSION_LATEST));
    for member in ["CreationTime", "LastModifiedTime"] {
        assert_offset_instant("ListFunctionUrlConfigs", member, &entry[member]);
    }

    // A layer version: `CreatedDate`, the member whose documentation spells the form.
    let layer = call(
        &router,
        "POST",
        &format!("{LAYERS_URI}/instant-layer/versions"),
        &json!({ "Content": { "ZipFile": package() } }),
    )
    .await;
    assert_eq!(
        layer.status,
        201,
        "PublishLayerVersion must succeed, got {} {}",
        layer.status,
        layer.text()
    );
    assert_offset_instant("PublishLayerVersion", "CreatedDate", &layer.member("CreatedDate"));

    // A code signing configuration: `LastModified`.
    let signing = call(
        &router,
        "POST",
        CODE_SIGNING_CONFIGS_URI,
        &json!({
            "AllowedPublishers": {
                "SigningProfileVersionArns": [format!(
                    "arn:aws:signer:{REGION}:{ACCOUNT}:/signing-profiles/Profile1/0000000a"
                )],
            },
        }),
    )
    .await;
    assert_eq!(
        signing.status,
        201,
        "CreateCodeSigningConfig must succeed, got {} {}",
        signing.status,
        signing.text()
    );
    assert_offset_instant(
        "CreateCodeSigningConfig",
        "LastModified",
        &signing.json()["CodeSigningConfig"]["LastModified"],
    );

    // A capacity provider: `LastModified`. The required `VpcConfig` names one subnet, of
    // the form the model's `SubnetId` pattern (`^subnet-[0-9a-z]*$`) admits.
    let provider = call(
        &router,
        "POST",
        CAPACITY_PROVIDERS_URI,
        &json!({
            "CapacityProviderName": "instant-provider",
            "VpcConfig": {
                "SubnetIds": ["subnet-10000000"],
                "SecurityGroupIds": [],
            },
            "PermissionsConfig": {
                "CapacityProviderOperatorRoleArn":
                    "arn:aws:iam::000000000000:role/service-role/lambda-capacity",
            },
        }),
    )
    .await;
    assert_eq!(
        provider.status,
        202,
        "CreateCapacityProvider must succeed, got {} {}",
        provider.status,
        provider.text()
    );
    assert_offset_instant(
        "CreateCapacityProvider",
        "LastModified",
        &provider.json()["CapacityProvider"]["LastModified"],
    );
}

/// `NextMarker` is a token the listing minted for the position a truncated page ended on:
/// it names none of the resources the API addresses, it walks the whole listing, and a
/// value the listing did not mint names no position at all.
#[tokio::test]
async fn a_page_marker_is_a_minted_token_the_listing_reads_back() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let function = "marker-probe";
    create_function(&router, function).await;
    // The qualifiers that can carry a function URL: the unpublished version and aliases,
    // created out of order so the listing's own order is asserted rather than the
    // creation order.
    let qualifiers = ["release", VERSION_LATEST, "beta", "Gamma_2"];
    for qualifier in qualifiers {
        if qualifier != VERSION_LATEST {
            create_alias(&router, function, qualifier).await;
        }
        let created = create_url_config(
            &router,
            function,
            Some(qualifier).filter(|named| *named != VERSION_LATEST),
            &json!({ "AuthType": "NONE" }),
        )
        .await;
        assert_eq!(
            created.status,
            201,
            "case {qualifier:?}: the function URL under test must be created, got {} {}",
            created.status,
            created.text()
        );
    }
    let mut ordered: Vec<String> = qualifiers.iter().map(|value| (*value).to_string()).collect();
    ordered.sort();
    let expected: Vec<String> = ordered
        .iter()
        .map(|qualifier| qualified_arn(function, qualifier))
        .collect();

    // Walked at every page size: every entry comes back exactly once, in order, and the
    // marker each page hands back is a value no resource of this API is addressed by.
    for page_size in [1_usize, 2, 3, qualifiers.len(), qualifiers.len() + 1] {
        let mut seen: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            pages += 1;
            assert!(
                pages <= qualifiers.len() + 1,
                "case page size {page_size}: paging did not terminate after {pages} pages"
            );
            let query = match &marker {
                Some(token) => format!("?MaxItems={page_size}&Marker={token}"),
                None => format!("?MaxItems={page_size}"),
            };
            let answer = list_url_configs(&router, function, &query).await;
            assert_eq!(
                answer.status,
                200,
                "case page size {page_size}: expected 200, got {} {}",
                answer.status,
                answer.text()
            );
            let page: Vec<String> = answer.json()["FunctionUrlConfigs"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|entry| entry["FunctionArn"].as_str().unwrap_or_default().to_string())
                .collect();
            assert!(
                page.len() <= page_size,
                "case page size {page_size}: expected at most {page_size} entries, got {}",
                page.len()
            );
            seen.extend(page);
            match answer.json()["NextMarker"].as_str() {
                Some(token) => {
                    for qualifier in &ordered {
                        assert_ne!(
                            token, qualifier,
                            "case page size {page_size}: a marker is a minted token, never a \
                             qualifier a caller addresses a resource by, got {token:?}"
                        );
                    }
                    for arn in &expected {
                        assert_ne!(
                            token, arn,
                            "case page size {page_size}: a marker is a minted token, never a \
                             resource ARN, got {token:?}"
                        );
                    }
                    marker = Some(token.to_string());
                }
                None => break,
            }
        }
        assert_eq!(
            seen, expected,
            "case page size {page_size}: expected every function url once in qualifier \
             order, got {seen:?}"
        );
    }

    // A page that reports every remaining entry is the last one, so it carries no marker.
    let whole = list_url_configs(&router, function, "").await;
    assert_eq!(
        whole.member("NextMarker"),
        Value::Null,
        "a page holding every entry carries no NextMarker, got {}",
        whole.text()
    );

    // A value this listing did not mint names no position: it is refused rather than
    // paged from, whether it is a qualifier, an ARN, a token of the right spelling or a
    // minted token with bytes added.
    let minted = list_url_configs(&router, function, "?MaxItems=1")
        .await
        .string("NextMarker")
        .unwrap_or_else(|| panic!("a truncated page must hand back a marker"));
    let forgeries = [
        ("a qualifier the function holds", "beta".to_string()),
        ("the unpublished version", VERSION_LATEST.to_string()),
        ("a resource ARN", expected[0].clone()),
        ("a minted token with bytes added", format!("{minted}AAAA")),
        ("a token of the right spelling", "A".repeat(64)),
        ("a value of another spelling", "marker.with.dots".to_string()),
    ];
    for (case, forged) in forgeries {
        let answer = list_url_configs(&router, function, &format!("?Marker={forged}")).await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?}: expected 400 for a marker this listing did not mint, got {} {}",
            answer.status,
            answer.text()
        );
        assert!(
            answer
                .error_code()
                .contains("InvalidParameterValueException"),
            "case {case:?}: a `Marker` carries no modelled constraint, so the refusal is \
             the modelled parameter error every paginated listing of this service answers \
             a marker that names no position with, got {}",
            answer.error_code()
        );
        assert!(
            answer.message().contains("marker"),
            "case {case:?}: the refusal must name the member it is about, got {:?}",
            answer.message()
        );
    }

    // The marker a page hands back resumes exactly after the entries it reported.
    let first = list_url_configs(&router, function, "?MaxItems=2").await;
    let token = first
        .string("NextMarker")
        .unwrap_or_else(|| panic!("a truncated page must hand back a marker"));
    let second = list_url_configs(&router, function, &format!("?Marker={token}")).await;
    let tail: Vec<String> = second.json()["FunctionUrlConfigs"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|entry| entry["FunctionArn"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        tail,
        expected[2..].to_vec(),
        "the marker must resume after the entries its page reported, got {tail:?}"
    );
}

/// A `Qualifier` outside the member's own modelled pattern is refused the way the
/// boundary walk refuses a pattern violation — 400 `ValidationException` over the clause
/// built from the request — on every function-URL operation that takes one.
#[tokio::test]
async fn a_qualifier_outside_the_modelled_pattern_is_refused_as_a_validation_error() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let function = "qualifier-probe";
    create_function(&router, function).await;
    create_alias(&router, function, "live").await;

    // The values the modelled `FunctionUrlQualifier` pattern
    // (`^((?!^\d+$)^[0-9a-zA-Z-_]+$)$`) excludes: an all-digit published version — in the
    // `Qualifier` parameter and in the `FunctionName` itself — and the unpublished
    // version named explicitly, whose `$` the character class does not admit.
    let refused: Vec<(&str, String, Option<&str>)> = vec![
        ("a version in the Qualifier parameter", function.to_string(), Some("1")),
        ("a multi-digit version", function.to_string(), Some("1024")),
        ("a zero-padded version", function.to_string(), Some("007")),
        ("a version in the function name", format!("{function}:1"), None),
        (
            "a version in a qualified function arn",
            format!("{}:1", qualified_arn(function, VERSION_LATEST)),
            None,
        ),
        (
            "the unpublished version named explicitly",
            function.to_string(),
            Some(VERSION_LATEST),
        ),
    ];

    for (case, reference, qualifier) in refused {
        for (operation, answer) in [
            (
                "CreateFunctionUrlConfig",
                create_url_config(&router, &reference, qualifier, &json!({ "AuthType": "NONE" }))
                    .await,
            ),
            (
                "GetFunctionUrlConfig",
                get_url_config(&router, &reference, qualifier).await,
            ),
            (
                "UpdateFunctionUrlConfig",
                update_url_config(&router, &reference, qualifier, &json!({ "AuthType": "NONE" }))
                    .await,
            ),
            (
                "DeleteFunctionUrlConfig",
                delete_url_config(&router, &reference, qualifier).await,
            ),
        ] {
            let label = format!("{operation} with {case}");
            assert_eq!(
                answer.status,
                400,
                "case {label:?}: expected 400 for a qualifier the modelled pattern excludes, \
                 got {} {}",
                answer.status,
                answer.text()
            );
            assert!(
                answer.error_code().contains("ValidationException"),
                "case {label:?}: a pattern violation is refused the way the boundary walk \
                 refuses one, so expected ValidationException, got {}",
                answer.error_code()
            );
            let message = answer.message();
            assert!(
                message.contains("at 'qualifier'")
                    && message.contains("Member must satisfy regular expression pattern:"),
                "case {label:?}: the refusal must state the member and the expression the \
                 value has to satisfy, got {message:?}"
            );
        }
    }

    // The control: the qualifiers the pattern admits are served, so the refusal above is
    // about the value and not about the operations.
    for (case, qualifier) in [("the unpublished version", None), ("an alias", Some("live"))] {
        let created =
            create_url_config(&router, function, qualifier, &json!({ "AuthType": "NONE" })).await;
        assert_eq!(
            created.status,
            201,
            "case {case:?}: an admitted qualifier must be served, got {} {}",
            created.status,
            created.text()
        );
        let read = get_url_config(&router, function, qualifier).await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: the created function URL must read back, got {} {}",
            read.status,
            read.text()
        );
    }
}
