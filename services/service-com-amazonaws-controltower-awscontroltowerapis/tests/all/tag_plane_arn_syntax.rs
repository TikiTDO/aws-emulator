// SPDX-License-Identifier: CC0-1.0

//! How Control Tower's tag plane refuses a `resourceArn` it cannot read as an ARN, over
//! HTTP.
//!
//! The rule under test: a tag-plane request whose `resourceArn` path parameter is outside
//! the `Arn` shape the model declares for it is refused before any handler runs with HTTP
//! 400, the error code `BadRequestException` and the sentence for THAT parameter —
//! [`INVALID_RESOURCE_ARN`] — identically on every operation that binds
//! `/tags/{resourceArn}`, while every OTHER pre-handler refusal of this service keeps the
//! generic request-body sentence the service states when it names no parameter.
//!
//! Two narrower rules keep the first one from over-reaching, and are asserted here too:
//!
//! * only the `resourceArn` parameter takes that sentence: a tag-plane request whose BODY
//!   or whose other request-line member breaks a modelled constraint, and an ARN-typed
//!   member of an operation OUTSIDE the tag plane, keep the generic sentence;
//! * a `resourceArn` the model admits is not refused at all — it reaches the handler,
//!   which answers for the resource it names.
//!
//! Every bound and pattern written out by hand names its source: `resourceArn` is an `Arn`
//! (`@length(min 20, max 2048)`, `@pattern ^arn:aws[0-9a-zA-Z_\-:\/]+$`) bound
//! `@httpLabel` on `ListTagsForResource` (`GET /tags/{resourceArn}` -> 200),
//! `TagResource` (`POST` -> 204) and `UntagResource` (`DELETE` -> 204, with `tagKeys` an
//! `@httpQuery` list of `TagKey`, `@length(1..128)`) — from
//! `smithy_query(query="operation", operation="ListTagsForResource,TagResource,\
//! UntagResource")` and `smithy_query(query="shape", shape="Arn,TagKey")` on this crate's
//! `smithy-model/`.

use smithy_server_runtime::MultiServiceRouter;

use crate::front_door_validation::{arn_of_length, FRONT_DOOR_CODE, FRONT_DOOR_MESSAGE};
use crate::landing_zone_lifecycle::{call, dispatch, emulator, Answer};
use crate::resource_tags::{resources, tags_path};

/// The message a `resourceArn` the service cannot read as an ARN is refused with: the
/// sentence for that one parameter, which names neither the value the caller sent nor the
/// constraint it broke.
const INVALID_RESOURCE_ARN: &str = "Invalid input resource arn: bad syntax";

/// One tag-plane operation, as the request line and body it binds.
struct TagPlaneCall {
    /// The modelled operation, for the case label.
    operation: &'static str,
    method: &'static str,
    /// What follows the `/tags/{resourceArn}` path: the member this operation binds to the
    /// query string, where it binds one.
    query: &'static str,
    /// The request body, for the operation that carries one.
    body: Option<serde_json::Value>,
}

/// Every operation that binds `/tags/{resourceArn}`, so each row of a table below is
/// asserted on all three rather than on the one the issue cited.
fn tag_plane_calls() -> Vec<TagPlaneCall> {
    vec![
        TagPlaneCall {
            operation: "ListTagsForResource",
            method: "GET",
            query: "",
            body: None,
        },
        TagPlaneCall {
            operation: "TagResource",
            method: "POST",
            query: "",
            body: Some(serde_json::json!({ "tags": { "Owner": "platform" } })),
        },
        TagPlaneCall {
            operation: "UntagResource",
            method: "DELETE",
            query: "?tagKeys=Owner",
            body: None,
        },
    ]
}

/// Address `arn` with `request`.
async fn address(router: &MultiServiceRouter, request: &TagPlaneCall, arn: &str) -> Answer {
    dispatch(
        router,
        request.method,
        &format!("{}{}", tags_path(arn), request.query),
        request.body.clone(),
    )
    .await
}

/// The `message` member of an answer's body, as the string a caller reads.
fn message(answer: &Answer) -> String {
    answer.string("message")
}

/// The `__type` member restJson1 names the error shape with in the body.
fn wire_type(answer: &Answer) -> String {
    answer.string("__type")
}

/// The ways a value can fall outside `Arn`: rows differ along WHAT about the value the
/// shape does not admit — its length against each published bound, the literal prefix the
/// pattern requires, and a character the pattern's class does not hold — and along the
/// value's length and form.
fn values_outside_the_arn_shape() -> Vec<(&'static str, String)> {
    vec![
        (
            "not an ARN at all, long enough to clear the length min of 20",
            "totally-not-an-arn-but-long-enough".to_string(),
        ),
        (
            "a single word with no separator at all",
            "x".repeat(40),
        ),
        (
            "the pattern's literal prefix misspelled",
            "brn:awsnotanarnatallxxxxx".to_string(),
        ),
        (
            "the pattern's literal prefix in upper case",
            "ARN:AWScontroltowerxxxxxx".to_string(),
        ),
        (
            "a dot, which the pattern's character class does not hold",
            "arn:aws.not.a.legal.arn.value".to_string(),
        ),
        (
            "a comma in the resource part",
            "arn:aws:controltower:us-west-2::enabledcontrol/abc,def".to_string(),
        ),
        (
            "the literal prefix alone, with nothing after it",
            "arn:aws".to_string(),
        ),
        (
            "one character below the length min of 20",
            arn_of_length(19),
        ),
        (
            "one character above the length max of 2048",
            arn_of_length(2049),
        ),
    ]
}

/// RULE: a `resourceArn` outside the `Arn` shape is refused with 400
/// `BadRequestException` and the sentence for that parameter, on every tag-plane
/// operation.
#[tokio::test]
async fn a_resource_arn_outside_the_arn_shape_is_refused_as_that_parameter() {
    let router = emulator();

    for (case, arn) in values_outside_the_arn_shape() {
        for request in tag_plane_calls() {
            let refused = address(&router, &request, &arn).await;
            assert_eq!(
                (refused.status, refused.error_code.as_deref()),
                (400, Some(FRONT_DOOR_CODE)),
                "case {case:?} on {}: expected 400 {FRONT_DOOR_CODE}, got {:?} body {:?}",
                request.operation,
                (refused.status, refused.error_code.as_deref()),
                refused.body
            );
            assert_eq!(
                wire_type(&refused),
                FRONT_DOOR_CODE,
                "case {case:?} on {}: expected __type {FRONT_DOOR_CODE}, got {:?}",
                request.operation,
                wire_type(&refused)
            );
            assert_eq!(
                message(&refused),
                INVALID_RESOURCE_ARN,
                "case {case:?} on {}: expected the resourceArn sentence, got {:?}",
                request.operation,
                message(&refused)
            );
        }
    }
}

/// RULE (how far it reaches): only the `resourceArn` parameter takes that sentence. A
/// refusal of anything else this service reads before a handler runs — a tag-plane body, a
/// tag-plane query parameter, an ARN-typed member of an operation outside the tag plane —
/// keeps the generic request-body sentence.
#[tokio::test]
async fn every_other_pre_handler_refusal_keeps_the_generic_sentence() {
    let router = emulator();

    // A `resourceArn` the model admits, so what each row below breaks is the only thing
    // the refusal can be about. Nothing minted it, but the refusal comes before the
    // handler that would answer 404 for it.
    let well_formed = "arn:aws:controltower:us-west-2:000000000000:enabledcontrol/absent";
    // `TagKey` is @length(min 1, max 128), so one character past its maximum is outside
    // the shape wherever the key travels.
    let over_long_key = "k".repeat(129);
    // `TagMap` is @length(min 0, max 200): one entry past its maximum.
    let over_sized_map: serde_json::Map<String, serde_json::Value> = (0..=200)
        .map(|index| {
            (
                format!("Key{index:04}"),
                serde_json::Value::String("v".to_string()),
            )
        })
        .collect();

    let rows: Vec<(&str, Answer)> = vec![
        (
            "a tag-plane BODY member outside TagMap's length max of 200",
            dispatch(
                &router,
                "POST",
                &tags_path(well_formed),
                Some(serde_json::json!({ "tags": over_sized_map })),
            )
            .await,
        ),
        (
            "a tag-plane QUERY member outside TagKey's length max of 128",
            dispatch(
                &router,
                "DELETE",
                &format!("{}?tagKeys={over_long_key}", tags_path(well_formed)),
                None,
            )
            .await,
        ),
        (
            "an ARN-typed member of an operation outside the tag plane",
            call(
                &router,
                "/get-enabled-control",
                serde_json::json!({ "enabledControlIdentifier": "totally-not-an-arn" }),
            )
            .await,
        ),
        (
            "a required member absent from an operation outside the tag plane",
            call(&router, "/get-enabled-baseline", serde_json::json!({})).await,
        ),
    ];

    for (case, refused) in rows {
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (400, Some(FRONT_DOOR_CODE)),
            "case {case:?}: expected 400 {FRONT_DOOR_CODE}, got {:?} body {:?}",
            (refused.status, refused.error_code.as_deref()),
            refused.body
        );
        assert_eq!(
            message(&refused),
            FRONT_DOOR_MESSAGE,
            "case {case:?}: expected the generic front-door sentence, got {:?}",
            message(&refused)
        );
    }
}

/// RULE (the other side of it): a `resourceArn` the `Arn` shape admits is not refused at
/// all. It reaches the handler, which answers for the resource it names — the modelled
/// success for an ARN this service minted, and the modelled `ResourceNotFoundException`
/// (404) for one it did not — and neither answer carries the ARN-syntax sentence.
#[tokio::test]
async fn a_resource_arn_the_model_admits_reaches_the_handler() {
    let router = emulator();
    let created = resources(&router).await;

    // (case, the value, the status each tag-plane operation answers it with)
    let mut rows: Vec<(String, String, bool)> = created
        .iter()
        .map(|resource| {
            (
                format!("the minted ARN of {}", resource.case),
                resource.arn.clone(),
                true,
            )
        })
        .collect();
    rows.extend([
        (
            "a well-formed ARN nothing minted".to_string(),
            "arn:aws:controltower:us-west-2:000000000000:enabledcontrol/absent".to_string(),
            false,
        ),
        (
            "a value exactly at the length min of 20".to_string(),
            arn_of_length(20),
            false,
        ),
        (
            "a value exactly at the length max of 2048".to_string(),
            arn_of_length(2048),
            false,
        ),
        (
            "an ARN of another AWS service".to_string(),
            "arn:aws:organizations::000000000000:ou/o-emulatororg/ou-abcd-9z".to_string(),
            false,
        ),
    ]);

    for (case, arn, minted) in rows {
        for request in tag_plane_calls() {
            let answer = address(&router, &request, &arn).await;
            // 200 for the read, 204 for the two writes: the statuses the model's `@http`
            // traits give them.
            let expected = if minted {
                (
                    if request.method == "GET" { 200 } else { 204 },
                    None,
                )
            } else {
                (404, Some("ResourceNotFoundException"))
            };
            assert_eq!(
                (answer.status, answer.error_code.as_deref()),
                expected,
                "case {case:?} on {}: expected {expected:?} — a value the model admits is \
                 the handler's to answer — got {:?} body {:?}",
                request.operation,
                (answer.status, answer.error_code.as_deref()),
                answer.body
            );
            assert_ne!(
                message(&answer),
                INVALID_RESOURCE_ARN,
                "case {case:?} on {}: a value the model admits must not be refused as bad \
                 ARN syntax, got {:?}",
                request.operation,
                answer.body
            );
        }
    }
}
