// SPDX-License-Identifier: CC0-1.0

//! WHICH characters the modelled `TagKey` character class admits, category by category.
//!
//! Every assertion goes through `MultiServiceRouter::dispatch`, so it exercises the same
//! awsJson1_1 path a real SDK takes.
//!
//! The rule under test: a modelled `@pattern` is applied exactly as declared, so a tag key
//! is admitted only when every one of its characters is in one of the three Unicode
//! general-category groups the class names (`\p{L}`, `\p{Z}`, `\p{N}`) or is one of the
//! eight characters it lists literally, and a key carrying anything else — including a
//! combining mark (`Mn`, `Mc`, `Me`), which no named group contains — is refused with
//! `ValidationException`. `tag_key_refusal` spans the refusal's wording and the operations
//! that state it; this suite spans the CLASS, one row per general category on each side of
//! it, so a character admitted by a wider approximation of `\p{L}` than the model declares
//! cannot pass unnoticed.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockdataautomationruntime_amazonbedrockkeystoneruntimeservice as bda;
use smithy_server_runtime::MultiServiceRouter;

/// This service's sigv4 signing name (`OPERATIONS.md`): the credential scope the router
/// routes on.
const SIGNING_NAME: &str = "bedrock";

/// The region every request below signs for.
const REGION: &str = "us-west-2";

/// A legal `dataAutomationProfileArn` (pattern from `smithy_query` shape
/// `DataAutomationProfileArn`, whose account segment accepts the literal `aws`).
const PROFILE_ARN: &str =
    "arn:aws:bedrock:us-west-2:aws:data-automation-profile/us.data-automation-v1";

/// A legal `dataAutomationConfiguration.dataAutomationProjectArn` (pattern from
/// `smithy_query` shape `DataAutomationProjectArn`): what tells the service WHAT to
/// extract, which every accepted submission has to name.
const PROJECT_ARN: &str =
    "arn:aws:bedrock:us-west-2:000000000000:data-automation-project/receipts";

/// The modelled `TagKey` pattern without the outer anchors a Smithy `@pattern` does not
/// need (`smithy_query` shape `TagKey`), as a refusal's requirement clause prints it.
const UNANCHORED_TAG_KEY_PATTERN: &str = r"(?!aws:)[\p{L}\p{Z}\p{N}_.:/=+\-@]*";

/// A router serving only this service over one shared engine, with [`PROJECT_ARN`]
/// arranged in the store of the service that OWNS data-automation projects: a submission
/// naming a project nobody created is refused before any rule this suite is about.
fn service() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    crate::support::seed_project(&engine, PROJECT_ARN, "tag-key-class-receipts");
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(bda::EmulatedService::new(engine));
    router
}

/// One wire answer: the HTTP status and the parsed JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The awsJson error code: the body's `__type`, local name only.
    fn error_code(&self) -> &str {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .split('#')
            .next_back()
            .unwrap_or_default()
    }

    /// The error message the body carries, or the empty string.
    fn message(&self) -> &str {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }
}

/// Call one operation on `router` the way an SDK would.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> Answer {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header(
            "x-amz-target",
            format!("AmazonBedrockKeystoneRuntimeService.{operation}"),
        )
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(&body).expect("serialize the request body"),
        ))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("a JSON response body")
    };
    Answer { status, body }
}

/// A submission carrying every required member at its minimum, optionally with `tags`.
fn submission(output: &str, tags: Option<Value>) -> Value {
    let mut body = json!({
        "inputConfiguration": {"s3Uri": "s3://in-bucket/assets/receipt.pdf"},
        "outputConfiguration": {"s3Uri": output},
        "dataAutomationProfileArn": PROFILE_ARN,
        "dataAutomationConfiguration": {"dataAutomationProjectArn": PROJECT_ARN},
    });
    if let Some(tags) = tags {
        body["tags"] = tags;
    }
    body
}

/// The tag keys the resource `arn` currently carries, sorted.
async fn tag_keys(router: &MultiServiceRouter, arn: &str) -> Vec<String> {
    let answer = call(router, "ListTagsForResource", json!({"resourceARN": arn})).await;
    assert_eq!(
        answer.status, 200,
        "the resource's tags must read back, got {} / {}",
        answer.status, answer.body
    );
    let mut keys: Vec<String> = answer.body["tags"]
        .as_array()
        .expect("the answer must carry a tags list")
        .iter()
        .map(|tag| {
            tag["key"]
                .as_str()
                .expect("every reported tag has a key")
                .to_string()
        })
        .collect();
    keys.sort();
    keys
}

/// One row of the class table: what makes the key interesting, the key, and whether the
/// modelled class admits every character in it.
struct Row {
    case: &'static str,
    key: &'static str,
    admitted: bool,
}

/// The characters the modelled class names, one row per Unicode general category on each
/// side of it.
///
/// SOURCE of the catalog: the `@pattern` of `smithy_query` shape `TagKey`,
/// `^(?!aws:)[\p{L}\p{Z}\p{N}_.:/=+\-@]*$`. Its class names three general-category groups
/// — `L` (`Lu`, `Ll`, `Lt`, `Lm`, `Lo`), `Z` (`Zs`, `Zl`, `Zp`) and `N` (`Nd`, `Nl`, `No`)
/// — plus eight literal characters, and NOTHING else: every other category (`M`, `P`, `S`,
/// `C`) is outside it, whichever Unicode properties its members also carry. Each row below
/// names the category of the character it turns on; the keys themselves are chosen here,
/// not taken from any one request.
const CLASS_ROWS: &[Row] = &[
    // The five letter categories `\p{L}` covers.
    Row { case: "an uppercase letter (Lu)", key: "OWNER", admitted: true },
    Row { case: "a lowercase letter with a precomposed diacritic (Ll)", key: "ümgebung", admitted: true },
    Row { case: "a titlecase letter (Lt)", key: "\u{01c5}group", admitted: true },
    Row { case: "a modifier letter (Lm)", key: "tier\u{02b0}", admitted: true },
    Row { case: "an other letter (Lo)", key: "請求書", admitted: true },
    // The three separator categories `\p{Z}` covers. No control character is among them,
    // so `White_Space` members like a tab are NOT admitted (a row below).
    Row { case: "a space separator (Zs)", key: "cost\u{a0}center", admitted: true },
    Row { case: "a line separator (Zl)", key: "cost\u{2028}center", admitted: true },
    Row { case: "a paragraph separator (Zp)", key: "cost\u{2029}center", admitted: true },
    // The three number categories `\p{N}` covers.
    Row { case: "a decimal digit (Nd)", key: "stage\u{663}", admitted: true },
    Row { case: "a letter number (Nl)", key: "phase\u{2160}", admitted: true },
    Row { case: "an other number (No)", key: "share\u{bc}", admitted: true },
    // Every character the class lists literally, in one key.
    Row { case: "all eight literal characters", key: "a_b.c:d/e=f+g-h@i", admitted: true },
    // `M` — combining marks. None of the three named groups contains one, so every mark is
    // outside the class, including the ones that carry `Other_Alphabetic` (and so are
    // `Alphabetic`, a property strictly wider than `\p{L}`).
    Row { case: "a spacing combining mark that is Other_Alphabetic (Mc)", key: "\u{0b95}\u{0bbe}", admitted: false },
    Row { case: "a nonspacing combining mark that is Other_Alphabetic (Mn)", key: "\u{0e01}\u{0e31}", admitted: false },
    Row { case: "a nonspacing combining mark that is not Other_Alphabetic (Mn)", key: "e\u{301}nv", admitted: false },
    Row { case: "an enclosing combining mark (Me)", key: "env\u{20dd}", admitted: false },
    // `P` — punctuation outside the eight literals.
    Row { case: "an other punctuation outside the literals (Po)", key: "100%done", admitted: false },
    Row { case: "an open punctuation (Ps)", key: "team[eu]", admitted: false },
    // `S` — symbols.
    Row { case: "a math symbol (Sm)", key: "budget\u{2264}cap", admitted: false },
    Row { case: "a currency symbol (Sc)", key: "budget$", admitted: false },
    Row { case: "an other symbol (So)", key: "env\u{1f680}", admitted: false },
    // `C` — controls and format characters.
    Row { case: "a control character that is White_Space (Cc)", key: "cost\tcenter", admitted: false },
    Row { case: "a format character (Cf)", key: "env\u{200d}ironment", admitted: false },
    // The lookahead half, which excludes exactly the reserved namespace: the same letters
    // without the colon, another case, and the prefix anywhere but the start are ordinary
    // keys.
    Row { case: "the reserved prefix exactly", key: "aws:", admitted: false },
    Row { case: "a key under the reserved prefix", key: "aws:cloudformation:stack-id", admitted: false },
    Row { case: "the reserved letters without the reserved colon", key: "awsome", admitted: true },
    Row { case: "the reserved prefix in another case", key: "AWS:team", admitted: true },
    Row { case: "the reserved prefix away from the start", key: "team:aws:owner", admitted: true },
];

/// The refusal message a malformed tag key at input path `path` must read as.
fn expected_refusal(path: &str) -> String {
    format!(
        "1 validation error detected: Value at '{path}' failed to satisfy constraint: \
         Member must satisfy regular expression pattern: {UNANCHORED_TAG_KEY_PATTERN}"
    )
}

#[tokio::test]
async fn the_modelled_tag_key_class_admits_only_the_categories_it_names() {
    let router = service();
    let answer = call(
        &router,
        "InvokeDataAutomationAsync",
        submission("s3://out-bucket/tag-key-class/", None),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "a well-formed submission must be accepted, got {} / {}",
        answer.status, answer.body
    );
    let arn = answer.body["invocationArn"]
        .as_str()
        .expect("the submission must answer with an ARN")
        .to_string();

    let mut admitted_keys: Vec<String> = Vec::new();

    for Row {
        case,
        key,
        admitted,
    } in CLASS_ROWS
    {
        // One row per operation that accepts a tag key, with the input path that
        // operation's request carries it at: the submission and `TagResource` carry whole
        // tags, `UntagResource` carries bare keys. All three are bound by the same
        // `TagKey` shape, so one character cannot be admitted by one and refused by
        // another.
        let operations: [(&str, Value, &str); 3] = [
            (
                "InvokeDataAutomationAsync",
                submission(
                    "s3://out-bucket/tag-key-class/",
                    Some(json!([{"key": key, "value": "v"}])),
                ),
                "tags.1.member.key",
            ),
            (
                "TagResource",
                json!({"resourceARN": arn, "tags": [{"key": key, "value": "v"}]}),
                "tags.1.member.key",
            ),
            (
                "UntagResource",
                json!({"resourceARN": arn, "tagKeys": [key]}),
                "tagKeys.1.member",
            ),
        ];
        for (operation, body, path) in operations {
            let answer = call(&router, operation, body).await;
            if *admitted {
                assert_eq!(
                    answer.status,
                    200,
                    "case {case:?} ({key:?}) on {operation}: every character is in a group \
                     the modelled TagKey class names, got {} / {}",
                    answer.status,
                    answer.body
                );
            } else {
                assert_eq!(
                    (answer.status, answer.error_code(), answer.message()),
                    (400, "ValidationException", expected_refusal(path).as_str()),
                    "case {case:?} ({key:?}) on {operation}: the key carries a character \
                     outside the modelled TagKey class, expected {:?}, got {} / {}",
                    (400, "ValidationException", expected_refusal(path)),
                    answer.status,
                    answer.body
                );
            }
        }
        if *admitted {
            admitted_keys.push((*key).to_string());
        }
    }

    // Nothing a refused row carried reached the tag map, and everything an admitted row
    // carried round-trips: the last `UntagResource` row removed each admitted key again,
    // so adding them all back is exactly what the resource must report.
    let answer = call(
        &router,
        "TagResource",
        json!({
            "resourceARN": arn,
            "tags": admitted_keys
                .iter()
                .map(|key| json!({"key": key, "value": "v"}))
                .collect::<Vec<Value>>(),
        }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "re-adding every admitted key must be accepted, got {} / {}",
        answer.status, answer.body
    );
    let mut expected = admitted_keys.clone();
    expected.sort();
    assert_eq!(
        tag_keys(&router, &arn).await,
        expected,
        "the resource must carry exactly the keys the modelled class admits, and no key \
         carrying a character outside it"
    );
}
