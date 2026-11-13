// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListFunctionUrlConfigs`: "Returns a list of Lambda function
//! URLs for the specified function."
//!
//! The rule under test: `ListFunctionUrlConfigs` reports, for the one function its
//! `FunctionName` resolves to, every function URL that function holds — one per qualifier
//! that has one, ordered by that qualifier, each entry the stored configuration
//! `GetFunctionUrlConfig` reports for the same qualifier — cut into pages of at most
//! `MaxItems` entries and never more than the documented fifty, with a `NextMarker` the
//! listing MINTS for the position a truncated page ended on, and it refuses a function it
//! does not hold and a `FunctionName` that qualifies one of the function's own
//! qualifiers.
//!
//! Every suite drives real HTTP requests through the router, so the assertions are about
//! the wire behaviour (status, body) rather than any handler's internals, and each listed
//! entry is checked against what `GetFunctionUrlConfig` reports for the same qualifier,
//! so nothing here trusts the listing alone. The rows differ along what the rule
//! generalises over: how many function URLs the function holds (none, one, several, more
//! than one page), which qualifiers hold them (the unpublished version, aliases, both),
//! how much of the configuration each URL carries (auth type, invoke mode, CORS), the
//! documented `FunctionName` form the request addresses the function by, the page size
//! and the marker the page continues from.

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// Where the function-URL operations are bound.
const URL_API_VERSION: &str = "/2021-10-31/functions";

/// The router picks a service by the SigV4 credential scope's service field, so every
/// request carries one naming `lambda` in `us-east-1`. Nothing in it is verified: this
/// harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The region the scope above names, which every minted ARN and function URL carries.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The unpublished version label, as the model's `Version` pattern
/// (`^(\$LATEST|[0-9]+)$`) spells it. A function URL on the unpublished version is the
/// one created by naming no `Qualifier` at all.
const VERSION_LATEST: &str = "$LATEST";

/// The bounds the model puts on this operation's `MaxItems` (`@range(min: 1, max: 50)`),
/// from `smithy_query(query="operation", operation="ListFunctionUrlConfigs")`.
const MAX_ITEMS_MIN: i64 = 1;
const MAX_ITEMS_MAX: i64 = 50;

/// The most entries one response carries, which the `MaxItems` member documents: "Note
/// that ListFunctionUrlConfigs returns a maximum of 50 items in each response, even if
/// you set the number higher."
const MAX_PAGE_SIZE: usize = 50;

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

    /// The body text, for a failure message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    /// The wire error code (`__type`, or the `x-amzn-errortype` header).
    fn error_code(&self) -> String {
        self.json()
            .get("__type")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                self.headers
                    .get("x-amzn-errortype")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| self.text())
    }
}

/// One service mounted on its own engine, reached exactly the way the production router
/// reaches it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // Pinned off so the suite does not depend on how the emulator process was started;
    // these tests assert Lambda's behaviour, not the emulator's SigV4 check.
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

/// The unqualified ARN a function of `name` carries.
fn function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")
}

/// The ARN of the qualifier `qualifier` of `function`: the unqualified ARN for the
/// unpublished version, the `:<qualifier>`-suffixed one otherwise.
fn qualified_arn(function: &str, qualifier: &str) -> String {
    if qualifier == VERSION_LATEST {
        function_arn(function)
    } else {
        format!("{}:{qualifier}", function_arn(function))
    }
}

/// The `Qualifier` query string a function-URL request carries for `qualifier`; empty for
/// the unpublished version, which is addressed by naming no qualifier.
fn qualifier_query(qualifier: &str) -> String {
    if qualifier == VERSION_LATEST {
        String::new()
    } else {
        format!("?Qualifier={qualifier}")
    }
}

/// `ListFunctionUrlConfigs` on the function `reference` names, with the given query
/// string.
async fn list_url_configs(router: &MultiServiceRouter, reference: &str, query: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("{URL_API_VERSION}/{reference}/urls{query}"),
        &Value::Null,
    )
    .await
}

/// `GetFunctionUrlConfig` for one qualifier of `function`.
async fn get_url_config(router: &MultiServiceRouter, function: &str, qualifier: &str) -> Answer {
    call(
        router,
        "GET",
        &format!(
            "{URL_API_VERSION}/{function}/url{}",
            qualifier_query(qualifier)
        ),
        &Value::Null,
    )
    .await
}

/// Create a function to attach function URLs to, publishing version 1 when asked.
async fn create_function_publishing(router: &MultiServiceRouter, name: &str, publish: bool) {
    let package: Vec<u8> = (0..64u16)
        .map(|byte| u8::try_from(byte % 251).unwrap_or(0))
        .collect();
    let created = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Publish": publish,
            "Code": { "ZipFile": base64::engine::general_purpose::STANDARD.encode(&package) },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {name}: CreateFunction must succeed, got {} {}",
        created.status,
        created.text()
    );
}

/// `CreateAlias` of `alias` on `function`, invoking `version`.
async fn create_alias(router: &MultiServiceRouter, function: &str, alias: &str, version: &str) {
    let created = call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{function}/aliases"),
        &json!({ "Name": alias, "FunctionVersion": version }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {function}/{alias}: CreateAlias must succeed, got {} {}",
        created.status,
        created.text()
    );
}

/// `CreateFunctionUrlConfig` on one qualifier of `function`, asserting it was created.
async fn create_url_config(
    router: &MultiServiceRouter,
    function: &str,
    qualifier: &str,
    body: &Value,
) {
    let created = call(
        router,
        "POST",
        &format!(
            "{URL_API_VERSION}/{function}/url{}",
            qualifier_query(qualifier)
        ),
        body,
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {function}/{qualifier}: CreateFunctionUrlConfig must succeed, got {} {}",
        created.status,
        created.text()
    );
}

/// The `FunctionUrlConfigs` array of a successful listing.
fn configs_of(case: &str, answer: &Answer) -> Vec<Value> {
    assert_eq!(
        answer.status,
        200,
        "case {case}: expected 200 from ListFunctionUrlConfigs, got {} {}",
        answer.status,
        answer.text()
    );
    let body = answer.json();
    body["FunctionUrlConfigs"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case}: expected a FunctionUrlConfigs array, got {body:?}")
        })
        .clone()
}

/// The `FunctionArn` of each entry of a listing page, which is the qualifier each entry
/// reports its function URL for.
fn arns_of(entries: &[Value]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| {
            entry["FunctionArn"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// One function URL to create: the qualifier it belongs to and the configuration body.
struct UrlOnQualifier {
    qualifier: String,
    /// The version an alias qualifier invokes; `None` for the unpublished version.
    alias_of: Option<&'static str>,
    request: Value,
}

/// One row of the listing table: a function, whether it publishes a version, and the
/// function URLs to create on it.
struct ListingCase {
    case: &'static str,
    function: &'static str,
    published: bool,
    urls: Vec<UrlOnQualifier>,
}

/// The rows. They differ along how many function URLs the function holds, which
/// qualifiers hold them, and how much of the configuration each one carries. The
/// `AuthType` values are the whole `FunctionUrlAuthType` enum (`NONE | AWS_IAM`) and the
/// invoke modes the whole `InvokeMode` enum (`BUFFERED | RESPONSE_STREAM`) plus its
/// absence, both as `smithy_query(query="shape", shape="FunctionUrlAuthType,InvokeMode")`
/// lists them.
fn listing_cases() -> Vec<ListingCase> {
    vec![
        // A function nobody gave a URL: an empty list, not a refusal.
        ListingCase {
            case: "no function urls at all",
            function: "urlless",
            published: false,
            urls: Vec::new(),
        },
        // The unpublished version alone, configured with nothing beyond its auth type.
        ListingCase {
            case: "one url on the unpublished version",
            function: "one-url",
            published: false,
            urls: vec![UrlOnQualifier {
                qualifier: VERSION_LATEST.to_string(),
                alias_of: None,
                request: json!({ "AuthType": "NONE" }),
            }],
        },
        // An alias alone: the unpublished version holds no URL, so the listing must not
        // invent one for it.
        ListingCase {
            case: "one url on an alias and none on the unpublished version",
            function: "alias-only-url",
            published: true,
            urls: vec![UrlOnQualifier {
                qualifier: "live".to_string(),
                alias_of: Some("1"),
                request: json!({
                    "AuthType": "AWS_IAM",
                    "InvokeMode": "RESPONSE_STREAM",
                    "Cors": { "AllowOrigins": ["https://www.example.com"] },
                }),
            }],
        },
        // Several qualifiers, created in the reverse of the order they must be reported
        // in, so a listing that reports creation order fails here. The names span both
        // documented alias separators, mixed case and digits.
        ListingCase {
            case: "urls on several qualifiers created out of order",
            function: "many-urls",
            published: true,
            urls: vec![
                UrlOnQualifier {
                    qualifier: "zulu".to_string(),
                    alias_of: Some("1"),
                    request: json!({ "AuthType": "NONE", "InvokeMode": "BUFFERED" }),
                },
                UrlOnQualifier {
                    qualifier: "Prod".to_string(),
                    alias_of: Some(VERSION_LATEST),
                    request: json!({
                        "AuthType": "AWS_IAM",
                        "Cors": {
                            "AllowCredentials": true,
                            "AllowHeaders": ["Date", "X-Custom-Header"],
                            "AllowMethods": ["GET", "POST"],
                            "AllowOrigins": ["https://www.example.com"],
                            "ExposeHeaders": ["Date"],
                            "MaxAge": 86_400,
                        },
                    }),
                },
                UrlOnQualifier {
                    qualifier: VERSION_LATEST.to_string(),
                    alias_of: None,
                    request: json!({ "AuthType": "NONE" }),
                },
                UrlOnQualifier {
                    qualifier: "stage_2".to_string(),
                    alias_of: Some("1"),
                    request: json!({
                        "AuthType": "NONE",
                        "InvokeMode": "RESPONSE_STREAM",
                        "Cors": { "AllowOrigins": ["*"], "MaxAge": 0 },
                    }),
                },
                UrlOnQualifier {
                    qualifier: "a".to_string(),
                    alias_of: Some("1"),
                    request: json!({ "AuthType": "AWS_IAM" }),
                },
            ],
        },
    ]
}

/// Set up one row's function, aliases and function URLs, and answer the qualifiers that
/// hold a URL in the order they must be reported in.
async fn set_up(router: &MultiServiceRouter, row: &ListingCase) -> Vec<String> {
    create_function_publishing(router, row.function, row.published).await;
    for url in &row.urls {
        if let Some(version) = url.alias_of {
            create_alias(router, row.function, &url.qualifier, version).await;
        }
        create_url_config(router, row.function, &url.qualifier, &url.request).await;
    }
    let mut qualifiers: Vec<String> = row.urls.iter().map(|url| url.qualifier.clone()).collect();
    qualifiers.sort();
    qualifiers
}

/// `ListFunctionUrlConfigs` reports every function URL of the named function, in
/// qualifier order, each entry exactly what `GetFunctionUrlConfig` reports for the same
/// qualifier.
#[tokio::test]
async fn list_function_url_configs_reports_every_stored_url_in_qualifier_order() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    for row in listing_cases() {
        let qualifiers = set_up(&router, &row).await;
        let expected: Vec<String> = qualifiers
            .iter()
            .map(|qualifier| qualified_arn(row.function, qualifier))
            .collect();

        // The same listing, addressed by each documented `FunctionName` form: the bare
        // name, the full function ARN and the partial ARN.
        for reference in [
            row.function.to_string(),
            function_arn(row.function),
            format!("{ACCOUNT}:function:{}", row.function),
        ] {
            let case = format!("{} addressed by {reference}", row.case);
            let answer = list_url_configs(&router, &reference, "").await;
            let entries = configs_of(&case, &answer);
            assert_eq!(
                arns_of(&entries),
                expected,
                "case {case}: expected every function url once in qualifier order, got {:?}",
                arns_of(&entries)
            );
            assert!(
                answer.json()["NextMarker"].is_null(),
                "case {case}: a listing that reported every function url is the last page, \
                 so it carries no NextMarker, got {:?}",
                answer.json()["NextMarker"]
            );
        }

        let entries = configs_of(row.case, &list_url_configs(&router, row.function, "").await);
        for (qualifier, entry) in qualifiers.iter().zip(&entries) {
            // The listed entry is the stored configuration, so every member of it is the
            // one `GetFunctionUrlConfig` reports for the same qualifier.
            let fetched = get_url_config(&router, row.function, qualifier).await;
            assert_eq!(
                fetched.status,
                200,
                "case {} qualifier {qualifier}: expected 200 from GetFunctionUrlConfig, got \
                 {} {}",
                row.case,
                fetched.status,
                fetched.text()
            );
            let configuration = fetched.json();
            for member in [
                "FunctionArn",
                "FunctionUrl",
                "AuthType",
                "InvokeMode",
                "Cors",
                "CreationTime",
                "LastModifiedTime",
            ] {
                assert_eq!(
                    entry[member], configuration[member],
                    "case {} qualifier {qualifier}: the listed {member} must be what \
                     GetFunctionUrlConfig reports ({:?}), got {:?}",
                    row.case, configuration[member], entry[member]
                );
            }
        }
    }
}

/// A paged walk reports every function URL of the function exactly once, in qualifier
/// order, whatever the page size — and the marker a page hands back continues after the
/// entries that page reported.
#[tokio::test]
async fn list_function_url_configs_pages_over_every_url() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let function = "paged-urls";
    create_function_publishing(&router, function, true).await;
    // Created out of order, each qualifier differing from the others in length, case and
    // the separator it carries; the unpublished version holds one too.
    let created = [
        "release-2",
        "beta",
        "Gamma",
        "a",
        "prod_blue",
        VERSION_LATEST,
        "zz-last",
        "mid_1",
    ];
    for qualifier in created {
        if qualifier != VERSION_LATEST {
            create_alias(&router, function, qualifier, "1").await;
        }
        create_url_config(&router, function, qualifier, &json!({ "AuthType": "NONE" })).await;
    }
    let mut expected: Vec<String> = created
        .iter()
        .map(|qualifier| (*qualifier).to_string())
        .collect();
    expected.sort();
    let expected_arns: Vec<String> = expected
        .iter()
        .map(|qualifier| qualified_arn(function, qualifier))
        .collect();

    // Page sizes that divide the entry count, that do not, that are the whole count, and
    // that are past it up to the modelled maximum.
    for page_size in [1_usize, 2, 3, 8, expected.len() + 1, MAX_PAGE_SIZE] {
        let mut seen: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(
                pages <= expected.len() + 2,
                "case page size {page_size}: paging did not terminate after {pages} pages"
            );
            let query = match &marker {
                Some(marker) => format!("?MaxItems={page_size}&Marker={marker}"),
                None => format!("?MaxItems={page_size}"),
            };
            let answer = list_url_configs(&router, function, &query).await;
            let page = configs_of(&format!("page size {page_size}"), &answer);
            assert!(
                page.len() <= page_size,
                "case page size {page_size}: expected at most {page_size} entries, got {}",
                page.len()
            );
            for arn in arns_of(&page) {
                assert!(
                    !seen.contains(&arn),
                    "case page size {page_size}: {arn} was reported twice"
                );
                seen.push(arn);
            }
            match answer.json()["NextMarker"].as_str() {
                Some(next) => marker = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, expected_arns,
            "case page size {page_size}: expected every function url once in qualifier order, \
             got {seen:?}"
        );
    }

    // The marker a page hands back continues AFTER the entries that page reported,
    // whichever position in the order that is: walked one entry at a time, the page that
    // follows each marker is the rest of the order.
    let mut marker: Option<String> = None;
    for (index, qualifier) in expected.iter().enumerate() {
        let answer = match &marker {
            Some(marker) => {
                list_url_configs(&router, function, &format!("?Marker={marker}")).await
            }
            None => list_url_configs(&router, function, "").await,
        };
        let arns = arns_of(&configs_of(&format!("after {index} entries"), &answer));
        let tail: Vec<String> = expected
            .iter()
            .skip(index)
            .map(|qualifier| qualified_arn(function, qualifier))
            .collect();
        assert_eq!(
            arns, tail,
            "case after {index} entries: expected the function urls from {qualifier:?} on, \
             got {arns:?}"
        );
        // One more entry's worth of the order, so the next pass continues from here.
        let page = list_url_configs(
            &router,
            function,
            &match &marker {
                Some(marker) => format!("?MaxItems=1&Marker={marker}"),
                None => "?MaxItems=1".to_string(),
            },
        )
        .await;
        marker = page.json()["NextMarker"].as_str().map(str::to_string);
        assert!(
            marker.as_deref().is_none_or(|minted| !expected
                .iter()
                .any(|qualifier| minted == qualifier.as_str())),
            "case after {index} entries: a marker is a token the listing minted for a \
             position, never a qualifier a caller can address a resource by, got {marker:?}"
        );
    }

    // A value this listing did not mint names no position in the order, so it is refused
    // rather than paged from — whether it is a qualifier, a value shaped like a token or
    // one of another listing's markers.
    let minted = list_url_configs(&router, function, "?MaxItems=1")
        .await
        .json()["NextMarker"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| panic!("a truncated page must hand back a marker"));
    for (case, marker) in [
        ("a qualifier", "mz".to_string()),
        ("a qualifier the function holds", "beta".to_string()),
        ("the unpublished version", VERSION_LATEST.to_string()),
        ("a marker with a byte appended", format!("{minted}AA")),
        ("an empty-looking token", "AAAAAAAAAAAA".to_string()),
    ] {
        let answer = list_url_configs(&router, function, &format!("?Marker={marker}")).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400 for a marker this listing did not mint, got {} {}",
            answer.status,
            answer.text()
        );
    }
}

/// "Note that ListFunctionUrlConfigs returns a maximum of 50 items in each response, even
/// if you set the number higher": a function holding more than one page of function URLs
/// is answered fifty at a time even when the caller names no `MaxItems`, and the marker
/// walks the rest.
#[tokio::test]
async fn list_function_url_configs_caps_a_page_at_the_documented_fifty() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let function = "crowded-urls";
    create_function_publishing(&router, function, true).await;
    let total = MAX_PAGE_SIZE + 13;
    let mut expected: Vec<String> = vec![VERSION_LATEST.to_string()];
    for index in 1..total {
        expected.push(format!("a{index:04}"));
    }
    for qualifier in &expected {
        if qualifier != VERSION_LATEST {
            create_alias(&router, function, qualifier, "1").await;
        }
        create_url_config(&router, function, qualifier, &json!({ "AuthType": "AWS_IAM" })).await;
    }
    expected.sort();
    let expected_arns: Vec<String> = expected
        .iter()
        .map(|qualifier| qualified_arn(function, qualifier))
        .collect();

    // No `MaxItems` at all: the documented ceiling still cuts the page, and the response
    // carries the marker the rest continues from — a token the listing minted, not the
    // qualifier the page ended on.
    let answer = list_url_configs(&router, function, "").await;
    let page = configs_of("unlimited page", &answer);
    assert_eq!(
        page.len(),
        MAX_PAGE_SIZE,
        "case unlimited page: expected the documented {MAX_PAGE_SIZE}-entry ceiling, got {}",
        page.len()
    );
    let minted = answer.json()["NextMarker"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| {
            panic!(
                "case unlimited page: a truncated page carries a marker, got {:?}",
                answer.json()["NextMarker"]
            )
        });
    assert!(
        !expected.contains(&minted),
        "case unlimited page: expected a minted pagination token rather than a qualifier \
         the caller can address a resource by, got {minted:?}"
    );

    // Walked through the marker, every function URL comes back exactly once, in order.
    let mut seen: Vec<String> = Vec::new();
    let mut marker: Option<String> = None;
    loop {
        let query = match &marker {
            Some(marker) => format!("?Marker={marker}"),
            None => String::new(),
        };
        let answer = list_url_configs(&router, function, &query).await;
        let page = configs_of("paged over a crowded function", &answer);
        assert!(
            page.len() <= MAX_PAGE_SIZE,
            "case paged over a crowded function: expected at most {MAX_PAGE_SIZE} entries, got {}",
            page.len()
        );
        seen.extend(arns_of(&page));
        match answer.json()["NextMarker"].as_str() {
            Some(next) => marker = Some(next.to_string()),
            None => break,
        }
    }
    assert_eq!(
        seen, expected_arns,
        "case paged over a crowded function: expected every function url once in qualifier \
         order, got {} entries",
        seen.len()
    );
}

/// One row of the refusal table.
struct RefusalCase {
    case: &'static str,
    /// The function reference in the URI.
    reference: &'static str,
    /// The query string of the request.
    query: &'static str,
    status: u16,
    error_code: &'static str,
}

/// A request naming no function, or a page the operation cannot address, is refused with
/// the modelled error rather than answered with an empty page.
#[tokio::test]
async fn list_function_url_configs_refuses_a_request_it_cannot_serve() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    create_function_publishing(&router, "url-refusals", true).await;
    create_alias(&router, "url-refusals", "live", "1").await;
    create_url_config(
        &router,
        "url-refusals",
        "live",
        &json!({ "AuthType": "NONE" }),
    )
    .await;

    let cases = [
        RefusalCase {
            case: "unknown function",
            reference: "absent",
            query: "",
            status: 404,
            error_code: "ResourceNotFoundException",
        },
        RefusalCase {
            case: "unknown function by arn",
            reference: "arn:aws:lambda:us-east-1:000000000000:function:absent",
            query: "",
            status: 404,
            error_code: "ResourceNotFoundException",
        },
        // A function URL belongs to one qualifier of a function, and this operation lists
        // every function URL OF A FUNCTION, so a qualified `FunctionName` names no
        // function whose URLs to list.
        RefusalCase {
            case: "FunctionName qualified by an alias",
            reference: "url-refusals:live",
            query: "",
            status: 400,
            error_code: "InvalidParameterValueException",
        },
        RefusalCase {
            case: "FunctionName qualified by a version",
            reference: "url-refusals:1",
            query: "",
            status: 400,
            error_code: "InvalidParameterValueException",
        },
    ];

    for case in cases {
        let answer = list_url_configs(&router, case.reference, case.query).await;
        assert_eq!(
            answer.status, case.status,
            "case {}: expected {}, got {} {}",
            case.case,
            case.status,
            answer.status,
            answer.text()
        );
        assert!(
            answer.error_code().contains(case.error_code),
            "case {}: expected the {} error code, got {}",
            case.case,
            case.error_code,
            answer.error_code()
        );
    }

    // `MaxItems` is modelled `@range(min: 1, max: 50)`: a page size outside it is refused
    // rather than paged over, and each published bound is accepted.
    for max_items in [MAX_ITEMS_MIN - 1, MAX_ITEMS_MIN - 2, MAX_ITEMS_MAX + 1] {
        let answer =
            list_url_configs(&router, "url-refusals", &format!("?MaxItems={max_items}")).await;
        assert_eq!(
            answer.status,
            400,
            "case MaxItems {max_items}: expected 400, got {} {}",
            answer.status,
            answer.text()
        );
    }
    for max_items in [MAX_ITEMS_MIN, MAX_ITEMS_MAX] {
        let answer =
            list_url_configs(&router, "url-refusals", &format!("?MaxItems={max_items}")).await;
        let arns = arns_of(&configs_of(&format!("MaxItems {max_items}"), &answer));
        assert_eq!(
            arns,
            vec![qualified_arn("url-refusals", "live")],
            "case MaxItems {max_items}: expected the function's one function url, got {arns:?}"
        );
    }
}
