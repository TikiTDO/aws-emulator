// SPDX-License-Identifier: CC0-1.0

//! What the edge does with ONE VIEWER REQUEST before and after the origin answers it, end to
//! end over HTTP: which parts of the request take part in the CACHE KEY, which of them reach
//! the ORIGIN, and under which CONTENT CODING the answer is served.
//!
//! The rules under test, each over a table rather than over one configuration:
//!
//! * the CACHE KEY carries exactly what the behaviour's cache policy names — a query string
//!   the policy does not name cannot separate two requests, and one it does name cannot be
//!   shared between them. `QueryStringBehavior` decides which: `none` names nothing,
//!   `whitelist` the listed names, `allExcept` everything else, `all` the whole query string,
//!   which is keyed on AS THE VIEWER SENT IT, so the same parameters in another order are
//!   another object.
//! * the ORIGIN REQUEST carries the UNION of what the cache key names and what the origin
//!   request policy forwards — "the headers, cookies, and query strings that are included in
//!   the cache key are also included in requests that CloudFront sends to the origin" — and
//!   nothing else: a parameter neither policy names is withheld from the origin.
//! * a behaviour whose `Compress` is true serves an answer under the content coding the
//!   viewer negotiated, from the codings its cache policy enables
//!   (`EnableAcceptEncodingBrotli` / `EnableAcceptEncodingGzip`), and the bytes it serves
//!   really are that coding's — each row DECODES them and finds the origin's own document.
//!   The normalized `Accept-Encoding` is part of the cache key, so two viewers that
//!   negotiated different codings are two objects.
//!
//! ## The origin this suite points at, and how it REPORTS what it was asked
//!
//! The origin is an endpoint this emulator serves: this service's own control-plane host,
//! whose `GET /2020-05-31/distribution` answers a `DistributionList`. That answer is not
//! merely proof the origin was reached — it REPORTS the query string the origin received,
//! because `ListDistributions` reads its `MaxItems` and the `DistributionList` carries the
//! value it read, and because a `Marker` that is no distribution id is refused by that
//! operation with a `400`. So a row can assert what reached the origin from the origin's own
//! answer, without an origin written for the test.

use bytes::Bytes;
use smithy_server_runtime::MultiServiceRouter;

use super::distribution_lifecycle::{
    call, create_distribution, emulator, past_propagation, API_VERSION, HOST,
};

/// An origin this emulator SERVES: this service's own control-plane endpoint.
const SERVED_ORIGIN: &str = HOST;

/// The viewer path the served origin answers a `DistributionList` at.
const SERVED_PATH: &str = "/2020-05-31/distribution";

/// `X-Cache` for an answer the origin produced for this request.
const MISS: &str = "Miss from cloudfront";

/// `X-Cache` for an answer the edge cache held.
const HIT: &str = "Hit from cloudfront";

/// The `MaxItems` a `ListDistributions` that was asked for none reports — the service's own
/// page ceiling (`crate::state::MAX_ITEMS`), which is what the origin's answer carries when
/// the viewer's `MaxItems` did not reach it.
const DEFAULT_MAX_ITEMS: &str = "100";

/// What the edge answered one viewer request with: the status, the headers a viewer reads the
/// edge's decisions from, and the BYTES, which a compressed answer is only decodable from.
struct Viewed {
    status: u16,
    x_cache: String,
    content_encoding: Option<String>,
    content_type: Option<String>,
    body: Bytes,
}

impl Viewed {
    /// The answer's bytes as text, decoded under the coding it says they are in — which is
    /// what proves the bytes really are that coding's rather than mislabelled.
    fn decoded(&self, case: &str) -> String {
        let bytes: Vec<u8> = match self.content_encoding.as_deref() {
            None => self.body.to_vec(),
            Some("br") => {
                let mut decoded = Vec::new();
                brotli::BrotliDecompress(&mut std::io::Cursor::new(&self.body), &mut decoded)
                    .unwrap_or_else(|error| {
                        panic!("case {case:?}: the answer is labelled br but does not decode: {error}")
                    });
                decoded
            }
            Some("gzip") => {
                let mut decoder = flate2::read::GzDecoder::new(&self.body[..]);
                let mut decoded = Vec::new();
                std::io::Read::read_to_end(&mut decoder, &mut decoded).unwrap_or_else(|error| {
                    panic!("case {case:?}: the answer is labelled gzip but does not decode: {error}")
                });
                decoded
            }
            Some(other) => panic!("case {case:?}: the answer names an unexpected coding {other:?}"),
        };
        String::from_utf8_lossy(&bytes).to_string()
    }
}

/// Send one VIEWER request to `host`: no AWS credential, carrying `headers` beside the
/// `Host` — exactly as a browser reaches a distribution.
async fn view(
    router: &MultiServiceRouter,
    host: &str,
    path_and_query: &str,
    headers: &[(&str, &str)],
) -> Viewed {
    let mut builder = ::http::Request::builder()
        .method("GET")
        .uri(path_and_query)
        .header("host", host);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(Bytes::new())
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    };
    let x_cache = header("x-cache").unwrap_or_default();
    let content_encoding = header("content-encoding");
    let content_type = header("content-type");
    Viewed {
        status,
        x_cache,
        content_encoding,
        content_type,
        body: response.into_body(),
    }
}

/// One CloudFront name-list wrapper: its `Quantity` and its `Items`, each item in the `Name`
/// element the model gives `QueryStringNamesList`. An empty list leaves the wrapper out, as a
/// behaviour that names none carries none.
fn names(values: &[&str]) -> String {
    if values.is_empty() {
        return String::new();
    }
    let items: String = values
        .iter()
        .map(|value| format!("<Name>{value}</Name>"))
        .collect();
    format!(
        "<QueryStrings><Quantity>{}</Quantity><Items>{items}</Items></QueryStrings>",
        values.len()
    )
}

/// Create a cache policy whose query-string behaviour is `behavior` over `listed`, with the
/// TTLs `ttl` names (zero for a policy that caches nothing) and the two compression flags —
/// and answer its minted id.
async fn cache_policy(
    router: &MultiServiceRouter,
    name: &str,
    behavior: &str,
    listed: &[&str],
    ttl: i64,
    gzip: bool,
    brotli: bool,
) -> String {
    let body = format!(
        "<CachePolicyConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <Name>{name}</Name><DefaultTTL>{ttl}</DefaultTTL><MaxTTL>{ttl}</MaxTTL>\
         <MinTTL>0</MinTTL>\
         <ParametersInCacheKeyAndForwardedToOrigin>\
         <EnableAcceptEncodingGzip>{gzip}</EnableAcceptEncodingGzip>\
         <EnableAcceptEncodingBrotli>{brotli}</EnableAcceptEncodingBrotli>\
         <HeadersConfig><HeaderBehavior>none</HeaderBehavior></HeadersConfig>\
         <CookiesConfig><CookieBehavior>none</CookieBehavior></CookiesConfig>\
         <QueryStringsConfig><QueryStringBehavior>{behavior}</QueryStringBehavior>{}\
         </QueryStringsConfig>\
         </ParametersInCacheKeyAndForwardedToOrigin>\
         </CachePolicyConfig>",
        names(listed)
    );
    let created = call(
        router,
        "POST",
        &format!("/{API_VERSION}/cache-policy"),
        Some(&body),
        None,
    )
    .await;
    assert_eq!(
        created.status, 201,
        "creating the cache policy {name:?} answered {}: {}",
        created.status, created.raw
    );
    created
        .root()
        .find_text("Id")
        .expect("the created cache policy reports an id")
        .to_string()
}

/// Create an origin request policy whose query-string behaviour is `behavior` over `listed`,
/// and answer its minted id.
async fn origin_request_policy(
    router: &MultiServiceRouter,
    name: &str,
    behavior: &str,
    listed: &[&str],
) -> String {
    let body = format!(
        "<OriginRequestPolicyConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <Name>{name}</Name>\
         <HeadersConfig><HeaderBehavior>none</HeaderBehavior></HeadersConfig>\
         <CookiesConfig><CookieBehavior>none</CookieBehavior></CookiesConfig>\
         <QueryStringsConfig><QueryStringBehavior>{behavior}</QueryStringBehavior>{}\
         </QueryStringsConfig>\
         </OriginRequestPolicyConfig>",
        names(listed)
    );
    let created = call(
        router,
        "POST",
        &format!("/{API_VERSION}/origin-request-policy"),
        Some(&body),
        None,
    )
    .await;
    assert_eq!(
        created.status, 201,
        "creating the origin request policy {name:?} answered {}: {}",
        created.status, created.raw
    );
    created
        .root()
        .find_text("Id")
        .expect("the created origin request policy reports an id")
        .to_string()
}

/// Create one distribution over the served origin under `cache_policy_id`, optionally naming
/// an origin request policy and asking for automatic compression, let the clock deploy it,
/// and answer the domain name a viewer reaches it at.
async fn deployed(
    router: &MultiServiceRouter,
    reference: &str,
    cache_policy_id: &str,
    origin_request_policy_id: Option<&str>,
    compress: bool,
) -> String {
    let policy = origin_request_policy_id
        .map(|id| format!("<OriginRequestPolicyId>{id}</OriginRequestPolicyId>"))
        .unwrap_or_default();
    let config = format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items><Origin>\
         <Id>primary</Id><DomainName>{SERVED_ORIGIN}</DomainName>\
         <CustomOriginConfig><HTTPPort>80</HTTPPort><HTTPSPort>443</HTTPSPort>\
         <OriginProtocolPolicy>https-only</OriginProtocolPolicy></CustomOriginConfig>\
         </Origin></Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>primary</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>\
         <AllowedMethods><Quantity>2</Quantity>\
         <Items><Method>GET</Method><Method>HEAD</Method></Items>\
         <CachedMethods><Quantity>2</Quantity>\
         <Items><Method>GET</Method><Method>HEAD</Method></Items></CachedMethods>\
         </AllowedMethods>\
         <Compress>{compress}</Compress>\
         <CachePolicyId>{cache_policy_id}</CachePolicyId>{policy}\
         </DefaultCacheBehavior>\
         <Comment>edge request shaping</Comment>\
         <Enabled>true</Enabled>\
         </DistributionConfig>"
    );
    let created = create_distribution(router, &config).await;
    assert_eq!(
        created.status, 201,
        "case {reference:?}: creating the distribution answered {}: {}",
        created.status, created.raw
    );
    let id = created
        .root()
        .find_text("Id")
        .expect("the created distribution reports an id")
        .to_string();
    let domain = created
        .root()
        .find_text("DomainName")
        .expect("the created distribution reports a domain name")
        .to_string();
    ::tokio::time::sleep(past_propagation()).await;
    let read = call(
        router,
        "GET",
        &format!("/{API_VERSION}/distribution/{id}"),
        None,
        None,
    )
    .await;
    assert_eq!(
        read.root().find_text("Status"),
        Some("Deployed"),
        "case {reference:?}: the distribution did not reach Deployed: {}",
        read.raw
    );
    domain
}

/// RULE: the cache key carries exactly the query strings the behaviour's CACHE POLICY names,
/// so two requests the policy says are one object share one cached answer and two it says
/// are different objects do not.
///
/// The rows differ along what the rule generalises over: the `QueryStringBehavior` (every
/// value `CachePolicyQueryStringBehavior` admits — `none`, `whitelist`, `allExcept`, `all`),
/// which parameter the two requests differ in (a named one, an unnamed one), how many
/// parameters they carry, and — for `all`, which keys on the query string whole — the ORDER
/// the viewer sent the same parameters in.
#[tokio::test]
async fn the_cache_key_carries_only_the_query_strings_the_cache_policy_names() {
    let router = emulator();

    // (case, the behaviour, the names it lists, the first query, the second query, the
    // X-Cache the SECOND request must get)
    type Row = (
        &'static str,
        &'static str,
        &'static [&'static str],
        &'static str,
        &'static str,
        &'static str,
    );
    let cases: [Row; 8] = [
        (
            "none keys on no query string, so another value is the same object",
            "none",
            &[],
            "?a=1",
            "?a=2",
            HIT,
        ),
        (
            "whitelist keys on a listed parameter",
            "whitelist",
            &["a"],
            "?a=1",
            "?a=2",
            MISS,
        ),
        (
            "whitelist keys on nothing else",
            "whitelist",
            &["a"],
            "?a=1&b=1",
            "?a=1&b=2",
            HIT,
        ),
        (
            "allExcept keys on nothing it lists",
            "allExcept",
            &["b"],
            "?a=1&b=1",
            "?a=1&b=2",
            HIT,
        ),
        (
            "allExcept keys on everything else",
            "allExcept",
            &["b"],
            "?a=1&b=1",
            "?a=2&b=1",
            MISS,
        ),
        (
            "all keys on the same query string as one object",
            "all",
            &[],
            "?a=1&b=2",
            "?a=1&b=2",
            HIT,
        ),
        (
            "all keys on the query string as the viewer sent it, order and all",
            "all",
            &[],
            "?a=1&b=2",
            "?b=2&a=1",
            MISS,
        ),
        ("all keys on every parameter", "all", &[], "?a=1", "?a=2", MISS),
    ];

    for (index, (case, behavior, listed, first_query, second_query, expected)) in
        cases.into_iter().enumerate()
    {
        let reference = format!("key-{index}");
        // A policy that CACHES (a minute), so the second request can be a hit at all.
        let policy = cache_policy(&router, &reference, behavior, listed, 60, false, false).await;
        let domain = deployed(&router, &reference, &policy, None, false).await;

        let first = view(&router, &domain, &format!("{SERVED_PATH}{first_query}"), &[]).await;
        assert_eq!(
            (first.status, first.x_cache.as_str()),
            (200, MISS),
            "case {case:?}: the first request must reach the origin, got {} with X-Cache \
             {:?} ({})",
            first.status,
            first.x_cache,
            first.decoded(case)
        );
        let second = view(&router, &domain, &format!("{SERVED_PATH}{second_query}"), &[]).await;
        assert_eq!(
            (second.status, second.x_cache.as_str()),
            (200, expected),
            "case {case:?}: {second_query:?} after {first_query:?} expected X-Cache \
             {expected:?}, got {} with {:?}",
            second.status,
            second.x_cache
        );
        // Whichever way it was answered, the answer is the ORIGIN's own document.
        assert!(
            second.decoded(case).contains("DistributionList"),
            "case {case:?}: expected the origin's own bytes through the edge, got {:?}",
            second.decoded(case)
        );
    }
}

/// RULE: the origin receives the union of the query strings the cache key names and the ones
/// the ORIGIN REQUEST POLICY forwards, and no other — a parameter neither policy names is
/// withheld from it.
///
/// Observed from the ORIGIN's own answer: `ListDistributions` reports the `MaxItems` it was
/// asked for, so a `MaxItems` that reached it is visible in the `DistributionList`, and it
/// refuses a `Marker` that is no distribution id with a `400`, so a `Marker` that reached it
/// is visible as the status. The rows differ along which policy names the parameter (the
/// cache policy, the origin request policy, neither), the `QueryStringBehavior` each uses,
/// and which parameter is asked about.
#[tokio::test]
async fn the_origin_sees_only_the_query_strings_the_policies_name() {
    let router = emulator();
    // The viewer asks for one page and resumes from a marker no listing handed out; what the
    // origin does with each of them is what reports whether it received it.
    let viewer_query = "?MaxItems=1&Marker=not-a-distribution-id";

    // (case, the cache policy's behaviour and names, the origin request policy — `None` for a
    // behaviour that names none — the status the origin answers, the MaxItems its answer
    // reports)
    type Row = (
        &'static str,
        (&'static str, &'static [&'static str]),
        Option<(&'static str, &'static [&'static str])>,
        u16,
        &'static str,
    );
    let cases: [Row; 6] = [
        (
            "neither policy names anything: the origin sees no query string at all",
            ("none", &[]),
            None,
            200,
            DEFAULT_MAX_ITEMS,
        ),
        (
            "the origin request policy forwards the one parameter it lists",
            ("none", &[]),
            Some(("whitelist", &["MaxItems"])),
            200,
            "1",
        ),
        (
            "a parameter the cache key names also reaches the origin",
            ("whitelist", &["MaxItems"]),
            None,
            200,
            "1",
        ),
        (
            "all forwards every parameter the viewer sent",
            ("none", &[]),
            Some(("all", &[])),
            400,
            DEFAULT_MAX_ITEMS,
        ),
        (
            "allExcept withholds the parameter it lists and forwards the rest",
            ("none", &[]),
            Some(("allExcept", &["MaxItems"])),
            400,
            DEFAULT_MAX_ITEMS,
        ),
        (
            "the two policies' parameters reach the origin together",
            ("whitelist", &["Marker"]),
            Some(("whitelist", &["MaxItems"])),
            400,
            DEFAULT_MAX_ITEMS,
        ),
    ];

    for (index, (case, keyed, forwarded, status, max_items)) in cases.into_iter().enumerate() {
        let reference = format!("forward-{index}");
        // A policy that caches NOTHING (every TTL zero), so each request in this suite is
        // answered by the origin and the origin's answer is what is being read.
        let policy = cache_policy(&router, &reference, keyed.0, keyed.1, 0, false, false).await;
        let origin_policy = match forwarded {
            Some((behavior, listed)) => {
                Some(origin_request_policy(&router, &reference, behavior, listed).await)
            }
            None => None,
        };
        let domain = deployed(
            &router,
            &reference,
            &policy,
            origin_policy.as_deref(),
            false,
        )
        .await;

        let answer = view(
            &router,
            &domain,
            &format!("{SERVED_PATH}{viewer_query}"),
            &[],
        )
        .await;
        let body = answer.decoded(case);
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected the origin to answer {status}, got {} ({body})",
            answer.status
        );
        if status == 200 {
            assert!(
                body.contains(&format!("<MaxItems>{max_items}</MaxItems>")),
                "case {case:?}: expected the origin to report MaxItems {max_items}, got {body}"
            );
        } else {
            // The origin refused the marker, which is the only way its answer could be this:
            // the parameter reached it.
            assert!(
                body.contains("not-a-distribution-id"),
                "case {case:?}: expected the origin's own refusal of the marker, got {body}"
            );
        }
    }
}

/// RULE: a behaviour whose `Compress` is true serves the answer under the content coding the
/// viewer negotiated from the ones its cache policy enables, and the normalized
/// `Accept-Encoding` is part of the cache key — so a second viewer that negotiated the same
/// coding is served the cached object and one that negotiated another is not.
///
/// Every row DECODES the bytes it got under the coding the answer claims, and finds the
/// origin's own document: a labelled answer whose bytes are not that coding's fails here. The
/// rows differ along what the rule generalises over: which codings the policy enables, what
/// the viewer accepts (both, one, neither, a weighted refusal, nothing at all), whether the
/// behaviour asks for compression, and whether the object was already cached.
#[tokio::test]
async fn a_compressible_answer_is_served_under_the_coding_the_viewer_negotiated() {
    let router = emulator();
    // One policy, both codings enabled, caching for a minute — so one distribution's rows
    // also show what the normalized header does to the cache key.
    let both = cache_policy(&router, "encode-both", "none", &[], 60, true, true).await;
    let compressing = deployed(&router, "encode-both", &both, None, true).await;

    // (case, what the viewer accepts, the coding the answer carries, the X-Cache)
    let cases: [(&str, Option<&str>, Option<&str>, &str); 7] = [
        (
            "a viewer that accepts brotli",
            Some("br"),
            Some("br"),
            MISS,
        ),
        (
            "the same viewer again is served the cached object",
            Some("br"),
            Some("br"),
            HIT,
        ),
        (
            "a viewer that accepts both is a brotli viewer, which is the same object",
            Some("gzip, deflate, br"),
            Some("br"),
            HIT,
        ),
        (
            "a viewer that accepts only gzip is another object",
            Some("gzip"),
            Some("gzip"),
            MISS,
        ),
        (
            "a viewer that refused brotli with a weight is that same gzip object",
            Some("br;q=0, gzip"),
            Some("gzip"),
            HIT,
        ),
        (
            "a viewer that accepts neither coding is served the origin's own bytes",
            Some("identity"),
            None,
            MISS,
        ),
        (
            "a viewer that negotiated nothing has no coding to normalize",
            None,
            None,
            MISS,
        ),
    ];

    for (case, accepts, coding, x_cache) in cases {
        let headers: Vec<(&str, &str)> = accepts
            .map(|value| vec![("accept-encoding", value)])
            .unwrap_or_default();
        let answer = view(&router, &compressing, SERVED_PATH, &headers).await;
        assert_eq!(
            (
                answer.status,
                answer.content_encoding.as_deref(),
                answer.x_cache.as_str()
            ),
            (200, coding, x_cache),
            "case {case:?}: expected 200 under Content-Encoding {coding:?} with X-Cache \
             {x_cache:?}, got {} under {:?} with {:?}",
            answer.status,
            answer.content_encoding,
            answer.x_cache
        );
        let decoded = answer.decoded(case);
        assert!(
            decoded.contains("DistributionList"),
            "case {case:?}: the decoded answer must be the origin's own document, got \
             {decoded:?}"
        );
        // The answer keeps the media type the ORIGIN stated for the bytes, whatever coding
        // they travelled under.
        assert_eq!(
            answer.content_type.as_deref(),
            Some("application/xml"),
            "case {case:?}: expected the origin's own media type, got {:?}",
            answer.content_type
        );
    }

    // The two halves of the rule that need another configuration: a behaviour that asks for
    // NO compression serves the origin's bytes however the viewer negotiated, and a cache
    // policy that enables only one coding negotiates only that one.
    // (case, the codings the policy enables, Compress, what the viewer accepts, the coding
    // the answer carries)
    type EncodingRow = (
        &'static str,
        (bool, bool),
        bool,
        &'static str,
        Option<&'static str>,
    );
    let others: [EncodingRow; 4] = [
        (
            "a behaviour that asks for no compression",
            (true, true),
            false,
            "br, gzip",
            None,
        ),
        (
            "a policy that enables only gzip, to a viewer that accepts both",
            (true, false),
            true,
            "br, gzip",
            Some("gzip"),
        ),
        (
            "a policy that enables only brotli, to a viewer that accepts only gzip",
            (false, true),
            true,
            "gzip",
            None,
        ),
        (
            "a policy that enables neither, so the header is an ordinary one",
            (false, false),
            true,
            "br, gzip",
            None,
        ),
    ];
    for (index, (case, (gzip, brotli), compress, accepts, coding)) in
        others.into_iter().enumerate()
    {
        let reference = format!("encode-{index}");
        let policy = cache_policy(&router, &reference, "none", &[], 60, gzip, brotli).await;
        let domain = deployed(&router, &reference, &policy, None, compress).await;
        let answer = view(
            &router,
            &domain,
            SERVED_PATH,
            &[("accept-encoding", accepts)],
        )
        .await;
        assert_eq!(
            (answer.status, answer.content_encoding.as_deref()),
            (200, coding),
            "case {case:?}: expected 200 under Content-Encoding {coding:?}, got {} under \
             {:?}",
            answer.status,
            answer.content_encoding
        );
        assert!(
            answer.decoded(case).contains("DistributionList"),
            "case {case:?}: the decoded answer must be the origin's own document, got {:?}",
            answer.decoded(case)
        );
    }
}
