// SPDX-License-Identifier: CC0-1.0

//! The ONE message template the request-level refusal of an unparseable
//! `ResourceARNList` value renders, pinned as a template rather than as one
//! recorded sentence.
//!
//! The rule: a value an ARN-list parameter carries that cannot be parsed as an
//! ARN is refused with HTTP 400 and `InvalidParameterException`, and the message
//! is the offending value exactly as the request wrote it followed by fixed text
//! — so the only part of the message that varies with the request is the value,
//! and neither the member the value arrived in nor the operation it was sent to
//! appears in it.
//!
//! The rows below therefore vary the value's LENGTH (both published bounds of the
//! member shape among them), its byte content and its ARN-ish-ness, and send each
//! one to every operation of this service that takes an ARN list, asserting that
//! the message is `value` + the fixed text in every combination: stripping the
//! fixed text off the end has to give the request's value back, byte for byte.
//!
//! Every test drives the real wire path — the router, `POST /` with an
//! `X-Amz-Target` and an awsJson1_1 body — so what is asserted is what an SDK
//! client sees.
//!
//! The forms a value can take to be unparseable, and the positive control that
//! an ARN this endpoint serves is still tagged and read back, are the sibling
//! `arn_parameter_refusal_wording` suite's subject; this one is about the shape
//! of the message and the LENGTHS the rule speaks for, so its own positive
//! control is a parseable element at each length extreme.

use std::sync::Arc;

use service_com_amazonaws_resourcegroupstaggingapi_resourcegroupstaggingapi_20170126::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for, and therefore the Region the
/// endpoint serves.
const REGION: &str = "us-east-1";

/// The fixed text the refusal appends to the offending value. Everything after
/// the value is fixed: the value is the message's only variable part.
const FIXED_TEXT: &str = " is not a valid AmazonResourceName (ARN)";

/// The declared bounds of one ARN-list element.
///
/// Source: `smithy_query(query="shape",shape="ResourceARN")` — `@length 1..1011`,
/// `@pattern ^[\s\S]*$`, the member target of both `ResourceARNListForTagUntag`
/// and `ResourceARNListForGet`.
const RESOURCE_ARN_MAX_LENGTH: usize = 1011;

/// See [`RESOURCE_ARN_MAX_LENGTH`].
const RESOURCE_ARN_MIN_LENGTH: usize = 1;

/// Every operation of this service whose input carries an ARN list, so the rule
/// is asserted where it holds rather than only where the issue observed it.
/// Source: the `ResourceARNList` member of `TagResources`, `UntagResources`
/// (`ResourceARNListForTagUntag`) and `GetResources` (`ResourceARNListForGet`).
const ARN_LIST_OPERATIONS: [&str; 3] = ["TagResources", "UntagResources", "GetResources"];

/// A router carrying only this service, over its own fresh engine, so each test
/// starts from an empty store.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The tests exercise refusal behaviour, not SigV4: the `Authorization`
    // header is present because it is what routes an awsJson request to the
    // `tagging` signing name and names the Region the endpoint serves.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Call one operation and return its HTTP status and parsed JSON body.
async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("ResourceGroupsTaggingAPI_20170126.{operation}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/tagging/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("build the request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: serde_json::Value = if response.body().is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, parsed)
}

/// The error code an awsJson error body carries (`__type` minus its namespace).
fn error_code(body: &serde_json::Value) -> String {
    let raw = body["__type"]
        .as_str()
        .unwrap_or_else(|| panic!("an error body with no __type: {body}"));
    raw.rsplit(['#', ':']).next().unwrap_or(raw).to_string()
}

/// The message an error body carries, under either spelling of the member.
fn error_message(body: &serde_json::Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or_else(|| panic!("an error body with no message: {body}"))
        .to_string()
}

/// The request body one ARN-list operation needs around `arns`.
fn request_for(operation: &str, arns: &[&str]) -> serde_json::Value {
    match operation {
        "TagResources" => {
            serde_json::json!({ "ResourceARNList": arns, "Tags": { "Env": "prod" } })
        }
        "UntagResources" => serde_json::json!({ "ResourceARNList": arns, "TagKeys": ["Env"] }),
        "GetResources" => serde_json::json!({ "ResourceARNList": arns }),
        other => panic!("{other} takes no ResourceARNList"),
    }
}

/// A value of `length` characters that is not an ARN.
fn value_of(length: usize) -> String {
    "n".repeat(length)
}

/// The refusal renders the offending value and nothing else that varies: the
/// message with the fixed text stripped off its end is the request's value, byte
/// for byte, for every unparseable value and every ARN-list operation.
#[tokio::test]
async fn an_unparseable_value_is_rendered_as_the_value_plus_fixed_text() {
    let router = emulator();

    // Rows vary along length (the member's published minimum and maximum
    // included), byte content and how ARN-like the value looks. None of them is
    // the value the issue cited: that one is a further row of the same class.
    let values: Vec<(&str, String)> = vec![
        ("the shortest admitted value", value_of(RESOURCE_ARN_MIN_LENGTH)),
        ("a plain identifier", "template-plain-name".to_string()),
        ("the longest admitted value", value_of(RESOURCE_ARN_MAX_LENGTH)),
        ("a value carrying the fixed text itself", format!("decoy{FIXED_TEXT}")),
        ("a colon-separated non-ARN", "aws:s3:::template-bucket".to_string()),
        ("a quoted value", "\"arn:aws:s3:::template-bucket\"".to_string()),
        ("a value holding a newline", "arn\naws\ns3".to_string()),
        ("non-ASCII text", "идентификатор-ресурса".to_string()),
        ("an emoji", "🪣".to_string()),
    ];

    for (case, value) in &values {
        for operation in ARN_LIST_OPERATIONS {
            let (status, body) = call(&router, operation, request_for(operation, &[value])).await;
            assert_eq!(
                status, 400,
                "case {case} ({value:?})/{operation}: expected HTTP 400, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidParameterException",
                "case {case} ({value:?})/{operation}: got {body}"
            );
            let actual = error_message(&body);
            // The template: strip the fixed text and the request's own value is
            // what remains.
            let rendered_value = actual.strip_suffix(FIXED_TEXT).unwrap_or_else(|| {
                panic!(
                    "case {case} ({value:?})/{operation}: expected a message ending in \
                     {FIXED_TEXT:?}, got {actual:?}"
                )
            });
            assert_eq!(
                rendered_value, value,
                "case {case} ({value:?})/{operation}: expected the value verbatim before the \
                 fixed text, got {rendered_value:?} (whole message {actual:?})"
            );
            // Neither the member the value arrived in nor the operation it was
            // sent to is part of the template.
            assert!(
                !actual.contains("ResourceARNList") && !actual.contains(operation),
                "case {case} ({value:?})/{operation}: the template names neither the member nor \
                 the operation, got {actual:?}"
            );
        }
    }

    // The same value therefore gets the same message from every operation, and
    // two values differ in the message by exactly their own text.
    let first = value_of(RESOURCE_ARN_MIN_LENGTH);
    let second = "template-second-value".to_string();
    let mut messages: Vec<String> = Vec::new();
    for operation in ARN_LIST_OPERATIONS {
        let (_, body) = call(&router, operation, request_for(operation, &[&first])).await;
        messages.push(error_message(&body));
    }
    assert!(
        messages.windows(2).all(|pair| pair[0] == pair[1]),
        "case one value across every ARN-list operation: expected one message, got {messages:?}"
    );
    let (_, other) = call(
        &router,
        "TagResources",
        request_for("TagResources", &[&second]),
    )
    .await;
    assert_eq!(
        error_message(&other),
        format!("{second}{FIXED_TEXT}"),
        "case a second value: only the value changes with the request, got {:?}",
        error_message(&other)
    );
}

/// The rule covers the values the model admits: one character past the member's
/// published maximum is refused by the boundary's own constraint report, not by
/// the ARN template, so the template is not a catch-all for every bad element.
#[tokio::test]
async fn a_value_past_the_published_maximum_is_not_refused_by_the_arn_template() {
    let router = emulator();

    let too_long = value_of(RESOURCE_ARN_MAX_LENGTH + 1);
    for operation in ARN_LIST_OPERATIONS {
        let (status, body) = call(&router, operation, request_for(operation, &[&too_long])).await;
        assert_eq!(
            status, 400,
            "case one past the maximum/{operation}: expected HTTP 400, got {status} with {body}"
        );
        let actual = error_message(&body);
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case one past the maximum/{operation}: an element longer than the member's declared \
             maximum is a constraint violation, got {body}"
        );
        // The boundary's own report: the list member the element sits in, and
        // the declared maximum it broke.
        assert!(
            actual.contains("at 'resourceARNList.1.member'")
                && actual.ends_with(&format!(
                    "Member must have length less than or equal to {RESOURCE_ARN_MAX_LENGTH}"
                )),
            "case one past the maximum/{operation}: expected the constraint clause naming the \
             element's member and the declared maximum, got {actual:?}"
        );
    }
}

/// The elements this suite's refusals do NOT speak for: an element that is a
/// parseable ARN is tagged and read back, so the refusals above report the
/// elements that really are unparseable rather than everything.
///
/// The rows are the length extremes of a parseable element, which is this suite's
/// own subject and what the sibling suite's positive control does not cover: the
/// shortest ARN the parse admits, and one exactly at the member's published
/// maximum.
#[tokio::test]
async fn a_parseable_element_at_each_length_extreme_is_tagged_and_read_back() {
    let router = emulator();

    let shortest = "arn:aws:s3:::b".to_string();
    let longest = format!(
        "arn:aws:s3:::{}",
        "b".repeat(RESOURCE_ARN_MAX_LENGTH - "arn:aws:s3:::".len())
    );
    let served: &[(&str, &String)] = &[
        ("the shortest parseable ARN", &shortest),
        ("a parseable ARN at the member's maximum", &longest),
    ];

    for (case, arn) in served {
        assert!(
            arn.len() <= RESOURCE_ARN_MAX_LENGTH,
            "case {case}: the row has to stay inside the member's declared maximum, got {} \
             characters",
            arn.len()
        );
        let (status, body) = call(
            &router,
            "TagResources",
            request_for("TagResources", &[arn.as_str()]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status} with {body}"
        );
        assert_eq!(
            body["FailedResourcesMap"],
            serde_json::json!({}),
            "case {case}: expected no failed resource, got {body}"
        );

        let (status, page) = call(
            &router,
            "GetResources",
            request_for("GetResources", &[arn.as_str()]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: read-back expected HTTP 200, got {status} with {page}"
        );
        let mapping = page["ResourceTagMappingList"]
            .as_array()
            .unwrap_or_else(|| panic!("a GetResources body with no ResourceTagMappingList: {page}"))
            .iter()
            .find(|mapping| mapping["ResourceARN"] == serde_json::json!(arn.as_str()))
            .unwrap_or_else(|| panic!("case {case}: the ARN is not in the page: {page}"));
        assert_eq!(
            mapping["Tags"],
            serde_json::json!([{ "Key": "Env", "Value": "prod" }]),
            "case {case}: expected the tag the accepted call wrote, got {mapping}"
        );
    }
}
