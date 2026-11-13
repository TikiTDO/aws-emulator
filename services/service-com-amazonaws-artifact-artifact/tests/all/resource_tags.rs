// SPDX-License-Identifier: CC0-1.0

//! `ListTagsForResource` for AWS Artifact, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, the front
//! door, the `@httpLabel` binding, the handler, the persisted state and serialization —
//! the same path a client takes, and nothing reads the state struct directly, so an
//! answer that is not computed from persisted state cannot pass.
//!
//! The rules under test, asserted over TABLES rather than one recorded request:
//!
//! 1. The answer is a projection of the stored record's tags: exactly the tag set the
//!    create persisted, whatever its size and whatever form its keys and values take,
//!    and an EMPTY map for a resource carrying none — which is not the same answer as
//!    the member being absent.
//! 2. `resourceArn` names a resource of THIS account: an ARN this account holds no
//!    taggable resource for — not an ARN at all, another account's, another region's,
//!    another service's, or one naming a resource of this service that carries no tags —
//!    is refused the way the AWS front door refuses every target it cannot authorize,
//!    with HTTP 401 `UnauthorizedException`, and discloses nothing about the ARN.
//! 3. A read, and a refused write, change nothing a later caller sees: repeated reads
//!    answer the same tags, and the other reader of the same record agrees.
//!
//! Rows differ along what the rules generalise over: the number of tags, the length and
//! the character classes of the keys and values, the region the resource was created in,
//! and which field of the ARN is wrong.
//!
//! The WRITE side of the same tag set — `TagResource` and `UntagResource` — is
//! `resource_tag_writes.rs`.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_artifact_artifact::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region this suite signs for unless a case names another.
const REGION: &str = "eu-west-1";

/// A second region, so a case can create a resource under one region and address it
/// under another.
const OTHER_REGION: &str = "us-east-1";

/// An account id that is not the one this emulator serves.
const FOREIGN_ACCOUNT: &str = "111122223333";

/// This service's ARN namespace, which is also its signing name
/// (`smithy_query(query="service_info")`).
const ARN_SERVICE: &str = "artifact";

/// The front door's authorization refusal: status, wire error code, and the gateway's
/// own constant text. Spelled out here rather than imported from the crate so a wrong
/// refusal in `src/` cannot make this suite pass.
const UNAUTHORIZED: (u16, &str, &str) = (401, "UnauthorizedException", "Unauthorized");

/// The front door's request-body refusal.
const INVALID_BODY: (u16, &str, &str) = (400, "BadRequestException", "Invalid request body");

/// `TagsMap`'s published entry bound — `@length(max: 50)`
/// (`smithy_query(query="shape",shape="TagsMap")`).
const MAX_TAG_ENTRIES: usize = 50;

/// `TagKey`'s published ceiling — `@length(min: 1, max: 128)` (same query).
const MAX_TAG_KEY: usize = 128;

/// `TagValue`'s published ceiling — `@length(min: 0, max: 256)` (same query); the floor
/// of 0 is why the empty value is a row below.
const MAX_TAG_VALUE: usize = 256;

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 credential scope for `region` naming this service — what routes the request
/// and what region a created inquiry's ARN is built for.
fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/artifact/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// What the emulator answered: the status, the error-shape header a restJson1 client
/// resolves an error from, and the JSON body.
struct Answer {
    status: u16,
    errortype_header: Option<String>,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The error body's `message`, or the empty string when it carries none.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }
}

/// Dispatch one request through the router, signed for `region`.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map_or_else(Bytes::new, |value| Bytes::from(value.to_string()));
    let request = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "artifact.eu-west-1.amazonaws.com")
        .header("authorization", signed_for(region))
        .header("content-type", "application/json")
        .body(payload)
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let errortype_header = response
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
    Answer {
        status,
        errortype_header,
        body,
    }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    call_in(router, REGION, method, uri, body).await
}

/// `resourceArn` is an `@httpLabel` on `GET /tags/{resourceArn}`, and a non-greedy label
/// is ONE path segment: the `/` inside an ARN's resource field has to be written `%2F`
/// or it would split the segment. `:` is a legal path character, so it is sent as-is
/// here and [`fully_encoded`] covers the spelling an SDK sends.
fn label(resource_arn: &str) -> String {
    resource_arn.replace('/', "%2F")
}

/// The same ARN with every reserved character percent-encoded, as an SDK writes it.
fn fully_encoded(resource_arn: &str) -> String {
    resource_arn.replace(':', "%3A").replace('/', "%2F")
}

async fn list_tags(router: &MultiServiceRouter, resource_arn: &str) -> Answer {
    call(router, "GET", &format!("/tags/{}", label(resource_arn)), None).await
}

/// An ARN built field by field, so a row can make exactly one field wrong.
fn arn(partition: &str, service: &str, region: &str, account: &str, resource: &str) -> String {
    format!("arn:{partition}:{service}:{region}:{account}:{resource}")
}

/// Create a compliance inquiry carrying `tags` in `region`, and answer with its ARN.
///
/// The ARN is read off the create's own answer rather than built here: the tag read
/// under test has to resolve the same ARN the service minted.
async fn tagged_inquiry(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    name: &str,
    tags: &Value,
) -> String {
    let mut body = json!({
        "name": name,
        "inquiryContent": { "query": "Which controls does this report cover?" },
        "supportMode": "AI_ONLY",
    });
    if !tags.as_object().is_some_and(serde_json::Map::is_empty) {
        body["tags"] = tags.clone();
    }
    let created = call_in(
        router,
        region,
        "POST",
        "/v1/compliance-inquiry/create",
        Some(body),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "case {case:?}: CreateComplianceInquiry must accept the submission, got {} with {:?}",
        created.status, created.body
    );
    created.body["complianceInquirySummary"]["arn"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the create must answer the inquiry's ARN, got {:?}",
                created.body
            )
        })
        .to_string()
}

/// The inquiry id inside one of this service's compliance-inquiry ARNs.
fn inquiry_id_of(resource_arn: &str, case: &str) -> String {
    resource_arn
        .rsplit('/')
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: {resource_arn:?} carries no resource id"))
        .to_string()
}

/// The `tags` member of a `ListTagsForResource` answer. REQUIRED to be present: a
/// resource with no tags has an empty tag map, and defaulting an absent member to `{}`
/// here would assert nothing about which of the two the operation answers.
fn tags_of(answer: &Answer, case: &str) -> Value {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: ListTagsForResource must serve the resource, got {} with {:?}",
        answer.status, answer.body
    );
    let tags = answer.body.get("tags").cloned().unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected a tags member, got {:?}",
            answer.body
        )
    });
    assert!(
        tags.is_object(),
        "case {case:?}: expected tags to be a map, got {tags:?}"
    );
    tags
}

/// Assert `answer` is the refusal `expected` — status, wire error code, the
/// `x-amzn-errortype` header a restJson1 client resolves the shape from, and the
/// gateway's constant message — and that it discloses nothing about `resource_arn`.
fn assert_refused(case: &str, answer: &Answer, expected: (u16, &str, &str), resource_arn: &str) {
    let (status, code, message) = expected;
    assert_eq!(
        answer.status, status,
        "case {case:?}: expected HTTP {status} {code}, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        code,
        "case {case:?}: expected __type {code}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.errortype_header.as_deref(),
        Some(code),
        "case {case:?}: expected x-amzn-errortype {code}, got {:?}",
        answer.errortype_header
    );
    assert_eq!(
        answer.message(),
        message,
        "case {case:?}: expected the one class message {message:?}, got {:?}",
        answer.body
    );
    assert!(
        !answer.body.to_string().contains(resource_arn),
        "case {case:?}: the refusal must not echo {resource_arn:?}, got {:?}",
        answer.body
    );
}

/// A tag map from pairs.
fn tag_map(pairs: &[(String, String)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect::<Map<String, Value>>(),
    )
}

/// The tag sets this suite covers. The rows differ along everything the rule
/// generalises over: how many entries the map carries (none, one, several, the published
/// ceiling), how long the keys and values are (including each published bound), and
/// which character classes of `TagKey` / `TagValue`'s pattern they use.
fn tag_sets() -> Vec<(String, Value)> {
    let every_class: String = "aZ9 _.:/=+-@".to_string();
    let mut rows: Vec<(String, Value)> = vec![
        ("no tags at all".to_string(), json!({})),
        ("one tag".to_string(), json!({ "team": "compliance" })),
        (
            "three tags".to_string(),
            json!({ "team": "legal", "env": "prod", "owner": "a.b@c" }),
        ),
        (
            "a key and a value using every character class the pattern admits".to_string(),
            tag_map(&[(every_class.clone(), every_class)]),
        ),
        (
            "an empty value, the floor of TagValue's length".to_string(),
            json!({ "cost.centre": "" }),
        ),
        (
            "a key and a value at their published ceilings".to_string(),
            tag_map(&[("k".repeat(MAX_TAG_KEY), "v".repeat(MAX_TAG_VALUE))]),
        ),
    ];
    // The entry ceiling itself: one past it is refused, which the bound table covers.
    let full: Vec<(String, String)> = (0..MAX_TAG_ENTRIES)
        .map(|entry| (format!("key{entry}"), format!("value{entry}")))
        .collect();
    rows.push((
        format!("{MAX_TAG_ENTRIES} tags, the published entry ceiling"),
        tag_map(&full),
    ));
    rows
}

/// Rule 1 and rule 3: the answer is the stored tag set, the other reader of the same
/// record agrees, and reading twice answers the same thing.
#[tokio::test]
async fn list_tags_for_resource_reports_the_tags_the_create_persisted() {
    let router = emulator();

    for (index, (case, tags)) in tag_sets().into_iter().enumerate() {
        let name = format!("tag set {index}");
        let resource_arn = tagged_inquiry(&router, REGION, &case, &name, &tags).await;

        // The operation under test reports exactly the persisted set.
        assert_eq!(
            tags_of(&list_tags(&router, &resource_arn).await, &case),
            tags,
            "case {case:?}: ListTagsForResource must report the tags the create persisted"
        );

        // The other reader of the same record agrees, which is what says both answers
        // come from one stored record: it reports the member whatever the tag set, the
        // empty map included, so every set is compared member for member.
        let id = inquiry_id_of(&resource_arn, &case);
        let described = call(
            &router,
            "GET",
            &format!("/v1/compliance-inquiry/getMetadata?complianceInquiryId={id}"),
            None,
        )
        .await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: the inquiry must be readable, got {} with {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.body["tags"], tags,
            "case {case:?}: GetComplianceInquiryMetadata must report the same tags, got {:?}",
            described.body
        );

        // Rule 3: the read changed nothing, so the second answer is the first.
        assert_eq!(
            tags_of(&list_tags(&router, &resource_arn).await, &case),
            tags,
            "case {case:?}: a repeated ListTagsForResource must answer the same tags"
        );
    }
}

/// Rule 1, on the region the resource was created in: the ARN the service minted names
/// the resource whatever region the request was signed for, because the ARN is the key.
#[tokio::test]
async fn a_resource_is_listed_by_the_arn_its_create_minted_in_any_region() {
    let router = emulator();
    let tags = json!({ "scope": "per-account" });

    for region in [REGION, OTHER_REGION] {
        let case = format!("created in {region}");
        let resource_arn =
            tagged_inquiry(&router, region, &case, &format!("inquiry in {region}"), &tags).await;
        assert!(
            resource_arn.contains(&format!(":{region}:")),
            "case {case:?}: the minted ARN must name the region the request was signed for, \
             got {resource_arn:?}"
        );

        // Read under BOTH regions' credential scopes: the stored ARN is what resolves
        // the resource, so the region a tag read is signed for changes nothing.
        for signed in [REGION, OTHER_REGION] {
            let answer = call_in(
                &router,
                signed,
                "GET",
                &format!("/tags/{}", label(&resource_arn)),
                None,
            )
            .await;
            assert_eq!(
                tags_of(&answer, &case),
                tags,
                "case {case:?}: a read signed for {signed} must report the same tags"
            );
        }
    }
}

/// Rule 1, on the spelling of the label: the ARN an SDK percent-encodes whole names the
/// same resource as the one that leaves `:` alone.
#[tokio::test]
async fn a_fully_percent_encoded_resource_arn_names_the_same_resource() {
    let router = emulator();
    let tags = json!({ "spelling": "encoded" });
    let case = "a fully percent-encoded label";
    let resource_arn = tagged_inquiry(&router, REGION, case, "encoded label", &tags).await;

    let answer = call(
        &router,
        "GET",
        &format!("/tags/{}", fully_encoded(&resource_arn)),
        None,
    )
    .await;
    assert_eq!(
        tags_of(&answer, case),
        tags,
        "case {case:?}: both spellings of the label must resolve the same resource"
    );
}

/// Rule 2: an ARN this account holds no taggable resource for is refused 401, and the
/// refusal is the same whatever is wrong with it — so a tag read cannot be used to probe
/// for what another account, or another service, holds.
///
/// Every row here IS a well-formed ARN: a value the surface cannot read an ARN out of at
/// all is a request-shape failure refused 400 before this rule is reached, which
/// `tagging_arn_syntax.rs` pins.
#[tokio::test]
async fn an_arn_this_account_holds_no_taggable_resource_for_is_refused_with_401() {
    let router = emulator();
    let tags = json!({ "team": "compliance" });
    let resource_arn = tagged_inquiry(&router, REGION, "the held resource", "held", &tags).await;
    let id = inquiry_id_of(&resource_arn, "the held resource");

    // A report the catalog publishes: a resource of this service that exists, is
    // service-owned (its ARN carries no account) and carries no tags at all, so it is
    // not a taggable resource of this account either.
    let listed = call(&router, "GET", "/v1/report/list", None).await;
    assert_eq!(
        listed.status, 200,
        "ListReports must serve the catalog, got {} with {:?}",
        listed.status, listed.body
    );
    let report_arn = listed.body["reports"][0]["arn"]
        .as_str()
        .expect("a published report carries an ARN")
        .to_string();

    // Rows differ along WHICH field of the ARN is wrong — the partition, the service, the
    // region, the account, the resource type, the id — plus the two resources of this
    // service that carry no tags.
    let rows: Vec<(String, String)> = vec![
        (
            "another partition".to_string(),
            arn(
                "aws-cn",
                ARN_SERVICE,
                "cn-north-1",
                engine::emulator_account_id(),
                &format!("compliance-inquiry/{id}"),
            ),
        ),
        (
            "another service's ARN in this account".to_string(),
            arn(
                "aws",
                "ec2",
                REGION,
                engine::emulator_account_id(),
                &format!("instance/{id}"),
            ),
        ),
        (
            "this account in another region".to_string(),
            arn(
                "aws",
                ARN_SERVICE,
                OTHER_REGION,
                engine::emulator_account_id(),
                &format!("compliance-inquiry/{id}"),
            ),
        ),
        (
            "another account, an id this account does hold".to_string(),
            arn(
                "aws",
                ARN_SERVICE,
                REGION,
                FOREIGN_ACCOUNT,
                &format!("compliance-inquiry/{id}"),
            ),
        ),
        (
            "another account, an id nothing holds".to_string(),
            arn(
                "aws",
                ARN_SERVICE,
                REGION,
                FOREIGN_ACCOUNT,
                "compliance-inquiry/0123456789abcdef",
            ),
        ),
        (
            "an account id one digit from this one".to_string(),
            arn(
                "aws",
                ARN_SERVICE,
                REGION,
                "000000000001",
                &format!("compliance-inquiry/{id}"),
            ),
        ),
        (
            "an empty account segment".to_string(),
            arn(
                "aws",
                ARN_SERVICE,
                REGION,
                "",
                &format!("compliance-inquiry/{id}"),
            ),
        ),
        (
            "a resource type this service does not hold".to_string(),
            arn(
                "aws",
                ARN_SERVICE,
                REGION,
                engine::emulator_account_id(),
                &format!("inquiry/{id}"),
            ),
        ),
        (
            "a well-formed inquiry id that was never minted".to_string(),
            arn(
                "aws",
                ARN_SERVICE,
                REGION,
                engine::emulator_account_id(),
                "compliance-inquiry/compliance-inquiry-ffffffffffffffff",
            ),
        ),
        (
            "the held id with one extra character".to_string(),
            format!("{resource_arn}0"),
        ),
        (
            "a report the catalog publishes, which carries no tags".to_string(),
            report_arn,
        ),
    ];

    for (case, candidate) in &rows {
        let answer = list_tags(&router, candidate).await;
        assert_refused(case, &answer, UNAUTHORIZED, candidate);
    }

    // The no-leak invariant: a foreign ARN naming an id this account DOES hold is
    // refused identically to one of the same form naming nothing at all.
    let held = list_tags(
        &router,
        &arn(
            "aws",
            ARN_SERVICE,
            REGION,
            FOREIGN_ACCOUNT,
            &format!("compliance-inquiry/{id}"),
        ),
    )
    .await;
    let absent = list_tags(
        &router,
        &arn(
            "aws",
            ARN_SERVICE,
            REGION,
            FOREIGN_ACCOUNT,
            "compliance-inquiry/compliance-inquiry-ffffffffffffffff",
        ),
    )
    .await;
    assert_eq!(
        (held.status, &held.body),
        (absent.status, &absent.body),
        "a foreign ARN must be refused identically whether or not this account holds that id"
    );

    // Rule 3, and the positive control for the table: the resource this account does
    // hold is still served with its tags, so the refusals are about the ARN and not
    // about the operation.
    assert_eq!(
        tags_of(
            &list_tags(&router, &resource_arn).await,
            "after the refusals"
        ),
        tags,
        "a refused call must not disturb the stored tags"
    );
}

/// Rule 3, on a write the front door refuses: the tag set a caller lists is unchanged by
/// a `TagResource` whose body breaks a published bound of `TagsMap`.
#[tokio::test]
async fn a_tag_write_refused_at_the_bound_leaves_the_listing_unchanged() {
    let router = emulator();
    let tags = json!({ "team": "compliance" });
    let case = "a refused tag write";
    let resource_arn = tagged_inquiry(&router, REGION, case, "bounded write", &tags).await;

    // One row per published rule of `TagsMap` the request is refused at: the entry
    // ceiling, and the character pattern of a tag value. (The key and value LENGTH
    // ceilings are exercised AT the bound by the positive table above, where they
    // round-trip.)
    let mut too_many = Map::new();
    for entry in 0..=MAX_TAG_ENTRIES {
        too_many.insert(format!("key{entry}"), json!("value"));
    }
    let rows: Vec<(String, Value)> = vec![
        (
            format!("{} tags, one past the entry ceiling", MAX_TAG_ENTRIES + 1),
            Value::Object(too_many),
        ),
        (
            "a value outside the pattern".to_string(),
            json!({ "owner": "not<allowed>" }),
        ),
    ];

    for (row, bad_tags) in &rows {
        let answer = call(
            &router,
            "POST",
            &format!("/tags/{}", label(&resource_arn)),
            Some(json!({ "tags": bad_tags })),
        )
        .await;
        assert_refused(row, &answer, INVALID_BODY, &resource_arn);
        assert_eq!(
            tags_of(&list_tags(&router, &resource_arn).await, row),
            tags,
            "case {row:?}: a refused tag write must leave the listing unchanged"
        );
    }
}
