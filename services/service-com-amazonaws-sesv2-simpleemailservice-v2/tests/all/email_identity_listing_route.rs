// SPDX-License-Identifier: CC0-1.0

//! `ListEmailIdentities` on the method and URI real clients send it on, end to end
//! over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, deserialization, the handler and serialization
//! are all exercised.
//!
//! The rule these cases pin: an operation is served on the method and URI the
//! service's own clients address it with, and a paging member that form carries in
//! the query string reaches the handler exactly as the same member carried in the
//! body does. So `GET /v2/email/identities` enumerates the identities with the same
//! answer the modelled `POST /v2/email/list-identities` gives, honouring `PageSize`
//! and the opaque `NextToken` it hands back, while a neighbouring URI in the same
//! space keeps answering what it answered before.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// One answered request: the status, the error shape when there was one, and the
/// parsed body.
struct Reply {
    status: u16,
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    /// The modelled error code the response names, or an empty string on success.
    fn error_code(&self) -> String {
        let raw = self
            .error_type
            .as_deref()
            .or_else(|| self.body.get("__type").and_then(Value::as_str))
            .unwrap_or_default();
        let name = raw.split('#').next_back().unwrap_or(raw);
        name.split(':').next().unwrap_or(name).to_string()
    }

    /// The identity names the listing reported, in the order it reported them.
    fn listed(&self) -> Vec<String> {
        self.body
            .get("EmailIdentities")
            .and_then(Value::as_array)
            .map(|rows| {
                rows.iter()
                    .map(|row| {
                        row.get("IdentityName")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The `NextToken` the listing handed back, if any.
    fn next_token(&self) -> Option<String> {
        self.body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

/// The emulated service behind its router.
struct Ses {
    router: MultiServiceRouter,
}

impl Ses {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These cases exercise routing and handler behaviour, not SigV4.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        let payload = match &body {
            Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serialize body")),
            None => bytes::Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://email.us-east-1.amazonaws.com{path}"))
            .header("host", "email.us-east-1.amazonaws.com")
            .header("content-type", "application/json")
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR0000000/20260101/us-east-1/ses/aws4_request, SignedHeaders=host, Signature=00",
            )
            .body(payload)
            .expect("build request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_type,
            body,
        }
    }

    /// `CreateEmailIdentity`.
    async fn create(&self, identity: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/identities",
            Some(json!({"EmailIdentity": identity})),
        )
        .await
    }

    /// `ListEmailIdentities` as an SDK sends it: a GET over the identity collection
    /// with the paging members in the query string.
    async fn list_get(&self, query: &str) -> Reply {
        let path = if query.is_empty() {
            "/v2/email/identities".to_string()
        } else {
            format!("/v2/email/identities?{query}")
        };
        self.call("GET", &path, None).await
    }

    /// `ListEmailIdentities` on the route the checked-in model binds.
    async fn list_post(&self, body: Value) -> Reply {
        self.call("POST", "/v2/email/list-identities", Some(body))
            .await
    }
}

/// The identities every case lists, in the order the store keys them.
const IDENTITIES: [&str; 4] = [
    "alpha.example.com",
    "beta.example.com",
    "gamma@example.net",
    "mail.example.org",
];

/// Create every identity the cases list.
async fn seeded() -> Ses {
    let ses = Ses::new();
    for identity in IDENTITIES {
        let created = ses.create(identity).await;
        assert_eq!(
            created.status, 200,
            "case {identity:?}: expected the identity to be created, got {} {:?}",
            created.status, created.body
        );
    }
    ses
}

#[tokio::test]
async fn the_sdks_get_form_lists_the_identities() {
    let ses = seeded().await;
    let reply = ses.list_get("").await;
    assert_eq!(
        reply.status, 200,
        "case bare-get: expected 200 from GET /v2/email/identities, got {} {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.listed(),
        IDENTITIES.map(str::to_string).to_vec(),
        "case bare-get: expected every identity, got {:?}",
        reply.listed()
    );
    assert_eq!(
        reply.next_token(),
        None,
        "case bare-get: a listing that returned everything hands back no token, got {:?}",
        reply.next_token()
    );
}

#[tokio::test]
async fn the_get_form_answers_what_the_modelled_route_answers() {
    // Both wire forms of one operation, over the requests the paging members can
    // make: every identity, one page, a page the size of the whole list, and the
    // documented bound of `PageSize` ("at least 0, and can be no more than 1000").
    let cases: &[(&str, &str, Value)] = &[
        ("no paging members", "", json!({})),
        ("one identity a page", "PageSize=1", json!({"PageSize": 1})),
        ("two identities a page", "PageSize=2", json!({"PageSize": 2})),
        (
            "a page the size of the list",
            "PageSize=4",
            json!({"PageSize": 4}),
        ),
        (
            "a page larger than the list",
            "PageSize=100",
            json!({"PageSize": 100}),
        ),
        (
            "the largest page the operation documents",
            "PageSize=1000",
            json!({"PageSize": 1000}),
        ),
    ];

    for (case, query, body) in cases {
        let ses = seeded().await;
        let over_get = ses.list_get(query).await;
        let over_post = ses.list_post(body.clone()).await;
        assert_eq!(
            (over_get.status, over_get.listed(), over_get.next_token()),
            (
                over_post.status,
                over_post.listed(),
                over_post.next_token()
            ),
            "case {case:?}: expected the GET form to answer what the modelled route answers, got {} {:?} against {} {:?}",
            over_get.status,
            over_get.body,
            over_post.status,
            over_post.body
        );
    }
}

#[tokio::test]
async fn the_get_forms_token_pages_through_the_whole_list() {
    // The token the GET form hands back is a position in the same listing, so
    // following it walks every identity exactly once, whatever the page size.
    for page_size in [1_usize, 2, 3] {
        let ses = seeded().await;
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for request in 0..=IDENTITIES.len() {
            let query = match &token {
                None => format!("PageSize={page_size}"),
                Some(token) => format!("PageSize={page_size}&NextToken={}", encoded(token)),
            };
            let reply = ses.list_get(&query).await;
            assert_eq!(
                reply.status, 200,
                "case (page size {page_size}, request {request}): expected 200, got {} {:?}",
                reply.status, reply.body
            );
            let page = reply.listed();
            assert!(
                page.len() <= page_size,
                "case (page size {page_size}, request {request}): expected at most {page_size} identities, got {page:?}"
            );
            seen.extend(page);
            token = reply.next_token();
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            token, None,
            "case (page size {page_size}): expected the paging to end, got {token:?}"
        );
        assert_eq!(
            seen,
            IDENTITIES.map(str::to_string).to_vec(),
            "case (page size {page_size}): expected every identity exactly once, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_token_the_get_form_did_not_mint_is_refused() {
    let ses = seeded().await;
    for token in ["not-a-token", "YWJj", ""] {
        let reply = ses.list_get(&format!("NextToken={}", encoded(token))).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "BadRequestException"),
            "case {token:?}: expected 400 BadRequestException, got {} {:?}",
            reply.status,
            reply.body
        );
    }
}

#[tokio::test]
async fn a_page_size_outside_the_documented_range_is_refused() {
    let ses = seeded().await;
    // "The value you specify has to be at least 0, and can be no more than 1000":
    // each side of the range.
    for page_size in ["-1", "1001", "2000"] {
        let reply = ses.list_get(&format!("PageSize={page_size}")).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "BadRequestException"),
            "case {page_size:?}: expected 400 BadRequestException, got {} {:?}",
            reply.status,
            reply.body
        );
    }
    // A query value that is not a number at all is refused exactly as the same value
    // in the body of the modelled route is: the query member reaches the one
    // deserializer the body member reaches.
    let over_get = ses.list_get("PageSize=not-a-number").await;
    let over_post = ses.list_post(json!({"PageSize": "not-a-number"})).await;
    assert_eq!(
        (over_get.status, over_get.error_code()),
        (over_post.status, over_post.error_code()),
        "case not-a-number: expected the GET form to be refused as the modelled route is, got {} {:?} against {} {:?}",
        over_get.status,
        over_get.body,
        over_post.status,
        over_post.body
    );
    assert_eq!(
        over_get.status, 400,
        "case not-a-number: expected a terminal 400, got {} {:?}",
        over_get.status, over_get.body
    );
}

#[tokio::test]
async fn the_neighbouring_routes_in_the_same_space_still_answer() {
    let ses = seeded().await;
    // The GET form takes the identity COLLECTION; one identity under it is
    // `GetEmailIdentity`'s own route, and a deeper path is nobody's.
    let one = ses
        .call("GET", "/v2/email/identities/alpha.example.com", None)
        .await;
    assert_eq!(
        one.status, 200,
        "case get-one: expected GetEmailIdentity to still answer, got {} {:?}",
        one.status, one.body
    );
    assert_eq!(
        one.body.get("IdentityType").and_then(Value::as_str),
        Some("DOMAIN"),
        "case get-one: expected the identity's own attributes, got {:?}",
        one.body
    );

    let created = ses.create("delta.example.com").await;
    assert_eq!(
        created.status, 200,
        "case create: expected POST over the collection to still create, got {} {:?}",
        created.status, created.body
    );

    let unmodelled = ses.call("GET", "/v2/email/identities/a/b/c", None).await;
    assert_eq!(
        unmodelled.status, 501,
        "case unmodelled: expected a path no route binds to stay unimplemented, got {} {:?}",
        unmodelled.status, unmodelled.body
    );
}

/// `value` percent-encoded for a query string, the way an SDK sends it — the crate's one
/// encoder ([`crate::json_wire::query_component`]), so a token carrying base64 padding or
/// `+` arrives as it was minted.
fn encoded(value: &str) -> String {
    crate::json_wire::query_component(value)
}
