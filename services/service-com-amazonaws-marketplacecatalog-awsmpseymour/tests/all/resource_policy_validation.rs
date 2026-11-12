// SPDX-License-Identifier: CC0-1.0

//! What `PutResourcePolicy` accepts as a resource-based policy document, end to end over
//! HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the boundary validator, the handler, the persisted state and
//! serialization — the same path a client takes.
//!
//! The rule under test: a member documented as a policy document formatted in JSON carries
//! a document in the IAM JSON policy language — a policy object whose `Statement` is a
//! statement, or a non-empty list of them, each stating an `Effect` of `Allow` or `Deny`,
//! at least one action (`Action` or `NotAction`) and, because a RESOURCE-based policy
//! states who the permission is granted to, at least one principal (`Principal` or
//! `NotPrincipal`) — and any other text, however well it parses as JSON, is refused with
//! the operation's modelled `ValidationException` (HTTP 422) and attaches nothing.
//!
//! The accepted rows differ along the form a policy is written in (the presence and value
//! of `Version`, `Statement` as one object or a list, the statement count, the elements and
//! their complements, and the string/list/object forms each element takes). The refused
//! rows differ along which clause of the grammar is broken and the JSON type that breaks
//! it.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::state::{
    CHANGE_SET_APPLYING, CHANGE_SET_PREPARING,
};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The catalog every operation's `Catalog` member documents as its fixed value.
const CATALOG: &str = "AWSMarketplace";

/// A SigV4 credential scope naming this service's signing name (`aws-marketplace`).
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/aws-marketplace/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The account every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// The restJson1 document media type: `application/json`, for answers and refusals alike.
const JSON: &str = "application/json";

/// The document a resource carrying no resource-based policy reports: the empty JSON
/// policy document, which states no `Statement` and so grants what no attached policy
/// grants — and which this suite shows `PutResourcePolicy` refuses to store.
const EMPTY_POLICY_DOCUMENT: &str = "{}";

/// The policy-language versions an IAM JSON policy document may declare, written out here
/// from the IAM JSON policy reference rather than read from the crate, so an
/// implementation that publishes its own set cannot make this suite pass. The reference
/// documents exactly two: the current one and the legacy one.
const POLICY_VERSIONS: [&str; 2] = ["2012-10-17", "2008-10-17"];

/// How long after the start a change set has certainly reached `SUCCEEDED`: the two spans
/// the implementation publishes, plus a margin for a slow machine.
fn wait_for_success() {
    std::thread::sleep(CHANGE_SET_PREPARING + CHANGE_SET_APPLYING + Duration::from_millis(400));
}

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the status, the JSON body and the two headers a restJson1
/// client reads an error from.
struct Answer {
    status: u16,
    body: Value,
    content_type: String,
    errortype: String,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        let raw = self.body["__type"].as_str().unwrap_or_default();
        raw.rsplit('#').next().unwrap_or(raw)
    }

    /// The string at `pointer`, or the empty string.
    fn text(&self, pointer: &str) -> String {
        self.body
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The `Message` of a refusal, under either casing a protocol spells it.
    fn message(&self) -> String {
        let lower = self.text("/message");
        if lower.is_empty() {
            self.text("/Message")
        } else {
            lower
        }
    }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "catalog.marketplace.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_EU_WEST_1);
    if !payload.is_empty() {
        builder = builder.header("content-type", JSON);
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    let content_type = header("content-type");
    let errortype = header("x-amzn-errortype");
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer {
        status,
        body,
        content_type,
        errortype,
    }
}

async fn put_resource_policy(
    router: &MultiServiceRouter,
    resource_arn: &str,
    policy: &str,
) -> Answer {
    call(
        router,
        "POST",
        "/PutResourcePolicy",
        Some(json!({"ResourceArn": resource_arn, "Policy": policy})),
    )
    .await
}

async fn get_resource_policy(router: &MultiServiceRouter, resource_arn: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/GetResourcePolicy?resourceArn={resource_arn}"),
        None,
    )
    .await
}

async fn delete_resource_policy(router: &MultiServiceRouter, resource_arn: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/DeleteResourcePolicy?resourceArn={resource_arn}"),
        None,
    )
    .await
}

/// The ARN of an entity this emulator really holds: created through `StartChangeSet`
/// (`Intent=APPLY`, which defaults) and read back out of `DescribeEntity`, so every case
/// below addresses a resource the store holds under the ARN the service minted for it.
async fn created_entity_arn(router: &MultiServiceRouter, entity_type: &str) -> String {
    let started = call(
        router,
        "POST",
        "/StartChangeSet",
        Some(json!({
            "Catalog": CATALOG,
            "ChangeSet": [{
                "ChangeType": "CreateProduct",
                "Entity": {"Type": format!("{entity_type}@1.0")},
                "Details": json!({"ProductTitle": entity_type}).to_string(),
            }],
        })),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "StartChangeSet must answer 200, got {} with {:?}",
        started.status, started.body
    );
    let change_set_id = started.text("/ChangeSetId");
    wait_for_success();

    let listed = call(
        router,
        "POST",
        "/ListChangeSets",
        Some(json!({"Catalog": CATALOG})),
    )
    .await;
    let entity_id = listed
        .body
        .pointer("/ChangeSetSummaryList")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|item| item["ChangeSetId"] == json!(change_set_id))
        .and_then(|item| item["EntityIdList"][0].as_str().map(str::to_string))
        .unwrap_or_else(|| {
            panic!(
                "change set {change_set_id} must report its entity: {:?}",
                listed.body
            )
        });

    let entity = call(
        router,
        "GET",
        &format!("/DescribeEntity?catalog={CATALOG}&entityId={entity_id}"),
        None,
    )
    .await;
    assert_eq!(
        entity.status, 200,
        "the created entity must be readable, got {} with {:?}",
        entity.status, entity.body
    );
    let arn = entity.text("/EntityArn");
    assert!(
        !arn.is_empty(),
        "the created entity must be named by an ARN the service minted, got {:?}",
        entity.body
    );
    arn
}

/// One statement naming `elements` beside its `Effect`.
fn statement(effect: &str, elements: &str) -> String {
    format!("{{\"Effect\":\"{effect}\",{elements}}}")
}

/// A statement this service's own resource policies are written with: an action of this
/// service and the account root as the principal.
fn granting_statement() -> String {
    statement(
        "Allow",
        &format!(
            "\"Principal\":{{\"AWS\":\"arn:aws:iam::{ACCOUNT}:root\"}},\
             \"Action\":\"aws-marketplace:DescribeEntity\""
        ),
    )
}

/// The documents `PutResourcePolicy` accepts: `(case, document)`.
///
/// The rows differ along the form a policy may be written in — whether `Version` is
/// declared and which of [`POLICY_VERSIONS`] it names, whether `Statement` is one object or
/// a list, how many statements it holds, whether the elements are the plain ones or their
/// complements, and whether each element is a string, a list or the `{"AWS": …}` object a
/// `Principal` takes.
fn accepted_documents() -> Vec<(String, String)> {
    let one = granting_statement();
    let mut rows = vec![
        (
            "one statement in a list, with the current version".to_string(),
            format!("{{\"Version\":\"2012-10-17\",\"Statement\":[{one}]}}"),
        ),
        (
            "one statement written as the object itself".to_string(),
            format!("{{\"Version\":\"2012-10-17\",\"Statement\":{one}}}"),
        ),
        (
            "no Version at all, which a policy may omit".to_string(),
            format!("{{\"Statement\":[{one}]}}"),
        ),
        (
            "an Id beside the statement".to_string(),
            format!("{{\"Id\":\"catalog-policy\",\"Statement\":[{one}]}}"),
        ),
        (
            "three statements, allowing and denying".to_string(),
            format!(
                "{{\"Statement\":[{one},{},{}]}}",
                statement(
                    "Deny",
                    "\"Principal\":\"*\",\"Action\":[\"aws-marketplace:StartChangeSet\"]"
                ),
                statement(
                    "Allow",
                    "\"Principal\":{\"Service\":[\"marketplacecatalog.amazonaws.com\"]},\
                     \"Action\":\"aws-marketplace:*\""
                ),
            ),
        ),
        (
            "the complements of Action and Principal".to_string(),
            format!(
                "{{\"Statement\":[{}]}}",
                statement(
                    "Deny",
                    "\"NotPrincipal\":{\"AWS\":[\"arn:aws:iam::000000000000:root\"]},\
                     \"NotAction\":\"aws-marketplace:DeleteResourcePolicy\""
                )
            ),
        ),
        (
            "a statement carrying a Sid, a Resource and a Condition too".to_string(),
            format!(
                "{{\"Statement\":[{}]}}",
                statement(
                    "Allow",
                    "\"Sid\":\"one\",\"Principal\":\"*\",\
                     \"Action\":\"aws-marketplace:DescribeEntity\",\"Resource\":\"*\",\
                     \"Condition\":{\"StringEquals\":{\"aws:PrincipalOrgID\":\"o-abc123\"}}"
                )
            ),
        ),
    ];
    // Every version the policy language has, including the legacy one the issue's
    // documents never named.
    for version in POLICY_VERSIONS {
        rows.push((
            format!("the {version} policy version"),
            format!("{{\"Version\":\"{version}\",\"Statement\":[{one}]}}"),
        ));
    }
    rows
}

/// The documents `PutResourcePolicy` refuses: `(case, document)`. Each breaks one clause of
/// the grammar, and the rows span every clause and the JSON types that break it — a scalar,
/// an array, an object with no statement, a statement that is not an object, and a
/// statement missing each of its three required elements in turn.
fn refused_documents() -> Vec<(String, String)> {
    let one = granting_statement();
    vec![
        // Parseable JSON that is not a document at all.
        ("a bare number".to_string(), "5".to_string()),
        ("a bare float".to_string(), "-0.5".to_string()),
        ("a bare string".to_string(), "\"hello\"".to_string()),
        ("a bare boolean".to_string(), "true".to_string()),
        ("null".to_string(), "null".to_string()),
        ("an empty array".to_string(), "[]".to_string()),
        (
            "an array of statements with no document around them".to_string(),
            format!("[{one}]"),
        ),
        // A JSON object that is not a policy.
        ("an empty object".to_string(), "{}".to_string()),
        (
            "an object with members of its own and no Statement".to_string(),
            "{\"Version\":\"2012-10-17\",\"Effect\":\"Allow\"}".to_string(),
        ),
        (
            "a Statement that is an empty list".to_string(),
            "{\"Version\":\"2012-10-17\",\"Statement\":[]}".to_string(),
        ),
        (
            "a Statement that is a string".to_string(),
            "{\"Statement\":\"let me in\"}".to_string(),
        ),
        (
            "a Statement list holding a scalar".to_string(),
            "{\"Statement\":[42]}".to_string(),
        ),
        (
            "a version no policy language has".to_string(),
            format!("{{\"Version\":\"2024-01-01\",\"Statement\":[{one}]}}"),
        ),
        (
            "a Version that is not even a string".to_string(),
            format!("{{\"Version\":20121017,\"Statement\":[{one}]}}"),
        ),
        // A statement missing one of the three elements a resource-policy statement states.
        (
            "a statement with no Effect".to_string(),
            "{\"Statement\":[{\"Principal\":\"*\",\
             \"Action\":\"aws-marketplace:DescribeEntity\"}]}"
                .to_string(),
        ),
        (
            "an Effect that is neither Allow nor Deny".to_string(),
            format!(
                "{{\"Statement\":[{}]}}",
                statement(
                    "Maybe",
                    "\"Principal\":\"*\",\"Action\":\"aws-marketplace:DescribeEntity\""
                )
            ),
        ),
        (
            "a statement with no Action".to_string(),
            format!(
                "{{\"Statement\":[{}]}}",
                statement("Allow", "\"Principal\":\"*\"")
            ),
        ),
        (
            "a statement whose Action names nothing".to_string(),
            format!(
                "{{\"Statement\":[{}]}}",
                statement("Allow", "\"Principal\":\"*\",\"Action\":[]")
            ),
        ),
        (
            "a statement with no Principal".to_string(),
            format!(
                "{{\"Statement\":[{}]}}",
                statement("Allow", "\"Action\":\"aws-marketplace:DescribeEntity\"")
            ),
        ),
        (
            "a statement whose Principal names nothing".to_string(),
            format!(
                "{{\"Statement\":[{}]}}",
                statement(
                    "Allow",
                    "\"Principal\":{},\"Action\":\"aws-marketplace:DescribeEntity\""
                )
            ),
        ),
        (
            "a second statement that is not a policy statement".to_string(),
            format!("{{\"Statement\":[{one},{{\"Effect\":\"Allow\"}}]}}"),
        ),
        // Text that is not JSON at all: the same refusal, since it is no more a policy
        // document than the rows above.
        ("text that is not JSON".to_string(), "not a policy".to_string()),
        ("an unterminated object".to_string(), "{".to_string()),
        (
            "an unterminated string".to_string(),
            "\"unterminated".to_string(),
        ),
    ]
}

/// A document outside the resource-policy grammar is refused 422 `ValidationException`
/// naming the member, and attaches NOTHING: the resource carries the policy it carried
/// before the refused call, and a resource that carried none still has none to report.
#[tokio::test]
async fn a_document_that_is_not_a_resource_policy_is_refused_and_attaches_nothing() {
    let router = emulator();
    let arn = created_entity_arn(&router, "AmiProduct").await;

    for (case, document) in refused_documents() {
        let refused = put_resource_policy(&router, &arn, &document).await;
        assert_eq!(
            (
                refused.status,
                refused.error_code(),
                refused.content_type.as_str(),
                refused.errortype.as_str(),
            ),
            (422, "ValidationException", JSON, "ValidationException"),
            "case {case:?} ({document}): a document that is not a resource policy must be \
             refused 422 ValidationException as application/json, got {} {:?} with {:?}",
            refused.status,
            refused.error_code(),
            refused.body
        );
        assert!(
            refused.message().contains("Policy"),
            "case {case:?}: the refusal must name the member it rejected, got {:?}",
            refused.message()
        );

        // Nothing was stored: the resource still carries no policy, so the read answers
        // the EMPTY policy document rather than the document that was turned away.
        let read = get_resource_policy(&router, &arn).await;
        assert_eq!(
            (read.status, read.text("/Policy")),
            (200, EMPTY_POLICY_DOCUMENT.to_string()),
            "case {case:?}: a refused document must not become the resource's policy, got \
             {} with {:?}",
            read.status,
            read.body
        );
    }

    // The same rows against a resource that already carries a policy: the refusal leaves
    // that policy exactly as it was, so a rejected document replaces nothing either.
    let attached = granting_statement();
    let attached = format!("{{\"Version\":\"2012-10-17\",\"Statement\":[{attached}]}}");
    let put = put_resource_policy(&router, &arn, &attached).await;
    assert_eq!(
        put.status, 200,
        "attaching a resource policy must answer 200, got {} with {:?}",
        put.status, put.body
    );

    for (case, document) in refused_documents() {
        let refused = put_resource_policy(&router, &arn, &document).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (422, "ValidationException"),
            "case {case:?}: the refusal must not depend on what the resource already \
             carries, got {} with {:?}",
            refused.status,
            refused.body
        );

        let read = get_resource_policy(&router, &arn).await;
        assert_eq!(
            (read.status, read.text("/Policy")),
            (200, attached.clone()),
            "case {case:?}: a refused document must leave the attached policy alone, got {} \
             with {:?}",
            read.status,
            read.body
        );
    }
}

/// A document inside the grammar is accepted whatever form it is written in, is stored
/// verbatim and is what the next `GetResourcePolicy` reports — the round trip through the
/// implemented reader.
#[tokio::test]
async fn every_form_a_resource_policy_is_written_in_is_accepted_and_read_back() {
    let router = emulator();
    let arn = created_entity_arn(&router, "SaaSProduct").await;

    for (case, document) in accepted_documents() {
        let put = put_resource_policy(&router, &arn, &document).await;
        assert_eq!(
            (put.status, put.content_type.as_str(), &put.body),
            (200, JSON, &json!({})),
            "case {case:?} ({document}): a resource policy document must be accepted with \
             200 application/json and an empty body, got {} {:?} with {:?}",
            put.status,
            put.content_type,
            put.body
        );

        let read = get_resource_policy(&router, &arn).await;
        assert_eq!(
            (read.status, read.text("/Policy")),
            (200, document.clone()),
            "case {case:?}: the read must report the document that was attached, got {} \
             with {:?}",
            read.status,
            read.body
        );
    }

    // The accepted documents are not accepted because the resource already carried one:
    // after clearing, the first form is accepted again from an empty start.
    let cleared = delete_resource_policy(&router, &arn).await;
    assert_eq!(
        cleared.status, 200,
        "clearing the policy must answer 200, got {} with {:?}",
        cleared.status, cleared.body
    );
    let (case, document) = accepted_documents()
        .into_iter()
        .next()
        .expect("the accepted table has rows");
    let put = put_resource_policy(&router, &arn, &document).await;
    assert_eq!(
        put.status, 200,
        "case {case:?}: attaching after a clear must answer 200, got {} with {:?}",
        put.status, put.body
    );
    let read = get_resource_policy(&router, &arn).await;
    assert_eq!(
        (read.status, read.text("/Policy")),
        (200, document),
        "case {case:?}: the re-attached document must be what is reported, got {} with {:?}",
        read.status,
        read.body
    );
}
