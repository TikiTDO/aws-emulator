// SPDX-License-Identifier: CC0-1.0

//! The document this service answers a client error with.
//!
//! RULE under test: every refusal this service answers carries the whole error envelope a
//! client of Amazon IVS Chat reads a failure out of — the error's `Code`, its `Message`, the
//! modelled exception's remaining members under `Details`, and the `RequestID` of the call
//! that failed — beside the `restJson1` discriminator the protocol resolves an error shape
//! from (the `x-amzn-errortype` header and the body's `__type`) and the modelled members the
//! model itself declares; and every response names the request it answered in the
//! `x-amzn-requestid` header the model publishes for it
//! (`smithy_query(query="service_info")` — `cors.additionalExposedHeaders`).
//!
//! The two refusal classes the envelope carries are each a rule of their own:
//!
//! * a request naming a resource this service does not hold answers
//!   `ResourceNotFoundException` (404) whose `message` is `Resource: <the identifier the
//!   caller sent> not found`, whose `resourceId` is that same identifier echoed exactly as
//!   received, and whose `resourceType` names the KIND the identifier names — the modelled
//!   `ResourceType` value where the model declares one for that kind (`ROOM`,
//!   `smithy_query(query="shape", shape="com.amazonaws.ivschat#ResourceType")`) and
//!   otherwise the resource-type element of the ARN itself (`logging-configuration`);
//! * a request whose member value the model refuses answers `ValidationException` (400)
//!   whose `message` names the operation-qualified member path and the requirement it
//!   failed, whose `reason` is the modelled `FIELD_VALIDATION_FAILED`, and whose `fieldList`
//!   carries one entry per refused member — named by that same path and carrying that same
//!   sentence.
//!
//! Every table row is a different input along what the rules generalise over (the
//! operation, the kind of resource an identifier names, the member, the constraint class it
//! violates, and which side of a bound the value sits on), never a rename of one case. The
//! router, the signed request and the `/tags/{resourceArn}` label encoder are the ones
//! `validation_rules_and_tagging.rs` already owns.

use serde_json::{json, Map, Value};

use crate::validation_rules_and_tagging::{encode_label, Answer, Emulator};

/// The account every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";
/// The region this suite signs for.
const REGION: &str = "us-west-2";

/// The modelled `@pattern` of `RoomName` and of `LoggingConfigurationName`, which are the
/// same (`smithy_query(query="operation", operation="CreateRoom,CreateLoggingConfiguration")`).
const NAME_PATTERN: &str = "^[a-zA-Z0-9-_]*$";

/// The modelled `@length` maximum of those two name members.
const NAME_MAX_LENGTH: usize = 128;

/// The error document's members, as a client of this service reads a failure.
const CODE: &str = "Code";
const MESSAGE: &str = "Message";
const DETAILS: &str = "Details";
const REQUEST_ID: &str = "RequestID";

/// The `restJson1` discriminator and the text, under the names the protocol renderer and an
/// SDK error parser use.
const TYPE_KEY: &str = "__type";
const SDK_MESSAGE_KEY: &str = "message";

/// The response headers the model publishes for a failure and for every response.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";
const ERROR_TYPE_HEADER: &str = "x-amzn-errortype";
const ERROR_MESSAGE_HEADER: &str = "x-amzn-errormessage";

/// An identifier for a room that was never created.
fn absent_room() -> String {
    format!("arn:aws:ivschat:{REGION}:{ACCOUNT}:room/neverCreated1")
}

/// An identifier for a logging configuration that was never created.
fn absent_logging_configuration() -> String {
    format!("arn:aws:ivschat:{REGION}:{ACCOUNT}:logging-configuration/neverMade1")
}

/// The one message rule every not-found refusal of this service follows.
fn not_found_message(identifier: &str) -> String {
    format!("Resource: {identifier} not found")
}

/// The `Details` a not-found refusal carries: the identifier the request sent, and the kind
/// it named.
fn not_found_details(identifier: &str, resource_type: &str) -> Value {
    json!({ "resourceId": identifier, "resourceType": resource_type })
}

/// The `Details` a member refusal carries: the modelled reason, and one `fieldList` entry
/// naming the member under the same path the sentence names it by and repeating that
/// sentence.
fn field_details(member_path: &str, message: &str) -> Value {
    json!({
        "reason": "FIELD_VALIDATION_FAILED",
        "fieldList": [{ "name": member_path, "message": message }],
    })
}

/// What a row expects of the answer.
#[derive(Debug)]
struct Expected {
    status: u16,
    code: &'static str,
    message: String,
    details: Value,
}

/// One refused call: how it is addressed, and what it carries.
struct Call {
    method: &'static str,
    uri: String,
    body: String,
}

/// One of the operations the model binds to `POST /<Operation>`.
fn rpc(operation: &str, body: &Value) -> Call {
    Call {
        method: "POST",
        uri: format!("/{operation}"),
        body: body.to_string(),
    }
}

/// `GET /tags/{resourceArn}` — the tagging surface names its resource in the path.
fn list_tags(resource_arn: &str) -> Call {
    Call {
        method: "GET",
        uri: format!("/tags/{}", encode_label(resource_arn)),
        body: String::new(),
    }
}

/// `POST /tags/{resourceArn}`.
fn tag(resource_arn: &str) -> Call {
    Call {
        method: "POST",
        uri: format!("/tags/{}", encode_label(resource_arn)),
        body: json!({ "tags": { "env": "x" } }).to_string(),
    }
}

/// `DELETE /tags/{resourceArn}?tagKeys=…`.
fn untag(resource_arn: &str) -> Call {
    Call {
        method: "DELETE",
        uri: format!(
            "/tags/{}?tagKeys={}",
            encode_label(resource_arn),
            encode_label("env")
        ),
        body: String::new(),
    }
}

/// The document an answer carries.
fn document(answer: &Answer) -> Map<String, Value> {
    serde_json::from_slice(&answer.body).unwrap_or_else(|error| {
        panic!(
            "expected a JSON document, got {:?}: {error}",
            answer.text()
        )
    })
}

/// Assert the whole envelope of one refusal, and answer the request id it stated.
fn assert_envelope(case: &str, answer: &Answer, expected: &Expected) -> String {
    assert_eq!(
        answer.status, expected.status,
        "case {case:?}: expected HTTP {}, got {} {:?}",
        expected.status,
        answer.status,
        answer.text()
    );
    let document = document(answer);
    // The members a client of this service reads a failure out of.
    assert_eq!(
        document.get(CODE).and_then(Value::as_str),
        Some(expected.code),
        "case {case:?}: expected {CODE} {:?}, got {document:?}",
        expected.code
    );
    assert_eq!(
        document.get(MESSAGE).and_then(Value::as_str),
        Some(expected.message.as_str()),
        "case {case:?}: expected {MESSAGE} {:?}, got {document:?}",
        expected.message
    );
    assert_eq!(
        document.get(DETAILS),
        Some(&expected.details),
        "case {case:?}: expected {DETAILS} {:?}, got {document:?}",
        expected.details
    );
    let request_id = document
        .get(REQUEST_ID)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        !request_id.is_empty(),
        "case {case:?}: expected a {REQUEST_ID}, got {document:?}"
    );
    // The protocol's own discriminator and text, so an SDK built from the model still
    // classifies and describes the failure.
    assert_eq!(
        document.get(TYPE_KEY).and_then(Value::as_str),
        Some(expected.code),
        "case {case:?}: expected the {TYPE_KEY} discriminator, got {document:?}"
    );
    assert_eq!(
        document.get(SDK_MESSAGE_KEY).and_then(Value::as_str),
        Some(expected.message.as_str()),
        "case {case:?}: expected the text under {SDK_MESSAGE_KEY}, got {document:?}"
    );
    // The modelled exception's own members survive at the top level, under the names the
    // model declares, beside their copy under `Details`.
    for (member, value) in expected
        .details
        .as_object()
        .expect("a details object is a JSON object")
    {
        assert_eq!(
            document.get(member),
            Some(value),
            "case {case:?}: expected the modelled member {member:?} to carry {value:?}, got \
             {document:?}"
        );
    }
    // The headers the model publishes for a failure.
    assert_eq!(
        answer.header(ERROR_TYPE_HEADER),
        Some(expected.code),
        "case {case:?}: expected {ERROR_TYPE_HEADER} {:?}, got {:?}",
        expected.code,
        answer.header(ERROR_TYPE_HEADER)
    );
    assert_eq!(
        answer.header(ERROR_MESSAGE_HEADER),
        Some(expected.message.as_str()),
        "case {case:?}: expected {ERROR_MESSAGE_HEADER} to carry the text, got {:?}",
        answer.header(ERROR_MESSAGE_HEADER)
    );
    assert_eq!(
        answer.header(REQUEST_ID_HEADER),
        Some(request_id.as_str()),
        "case {case:?}: the header and the document must name one request, got {:?} and \
         {request_id:?}",
        answer.header(REQUEST_ID_HEADER)
    );
    request_id
}

/// Every row of the two refusal classes: the case, the call, and the envelope it owes.
fn rows() -> Vec<(String, Call, Expected)> {
    let room = absent_room();
    let configuration = absent_logging_configuration();
    let over_long_name = "a".repeat(NAME_MAX_LENGTH + 1);

    let not_found = |operation: &str, body: Value, identifier: &str, kind: &'static str| {
        (
            format!("{operation} on an absent {kind}"),
            rpc(operation, &body),
            Expected {
                status: 404,
                code: "ResourceNotFoundException",
                message: not_found_message(identifier),
                details: not_found_details(identifier, kind),
            },
        )
    };

    let mut rows = vec![
        // The not-found class, over every operation that names a resource by identifier —
        // rows differ in the operation, the member the identifier arrives in, and the KIND
        // of resource it names.
        not_found(
            "GetRoom",
            json!({ "identifier": room }),
            &room,
            "ROOM",
        ),
        not_found(
            "UpdateRoom",
            json!({ "identifier": room, "name": "renamed" }),
            &room,
            "ROOM",
        ),
        not_found("DeleteRoom", json!({ "identifier": room }), &room, "ROOM"),
        not_found(
            "CreateChatToken",
            json!({ "roomIdentifier": room, "userId": "u" }),
            &room,
            "ROOM",
        ),
        not_found(
            "GetLoggingConfiguration",
            json!({ "identifier": configuration }),
            &configuration,
            "logging-configuration",
        ),
        not_found(
            "UpdateLoggingConfiguration",
            json!({ "identifier": configuration, "name": "renamed" }),
            &configuration,
            "logging-configuration",
        ),
        not_found(
            "DeleteLoggingConfiguration",
            json!({ "identifier": configuration }),
            &configuration,
            "logging-configuration",
        ),
    ];

    // The tagging surface names its resource in the URI rather than in the body, and
    // reaches both kinds of ARN.
    for (case, call, identifier, kind) in [
        (
            "ListTagsForResource on an absent room",
            list_tags(&room),
            room.clone(),
            "ROOM",
        ),
        (
            "TagResource on an absent room",
            tag(&room),
            room.clone(),
            "ROOM",
        ),
        (
            "UntagResource on an absent room",
            untag(&room),
            room.clone(),
            "ROOM",
        ),
        (
            "ListTagsForResource on an absent logging configuration",
            list_tags(&configuration),
            configuration.clone(),
            "logging-configuration",
        ),
    ] {
        rows.push((
            case.to_string(),
            call,
            Expected {
                status: 404,
                code: "ResourceNotFoundException",
                message: not_found_message(&identifier),
                details: not_found_details(&identifier, kind),
            },
        ));
    }

    // The member-refusal class — rows differ in the operation, the member, the constraint
    // class (pattern, length, range) and which bound the value crossed.
    let field = |case: &str, operation: &str, body: Value, member: &str, requirement: String| {
        let member_path = format!("{operation}.{member}");
        let message = format!("{member_path} {requirement}");
        (
            case.to_string(),
            rpc(operation, &body),
            Expected {
                status: 400,
                code: "ValidationException",
                message: message.clone(),
                details: field_details(&member_path, &message),
            },
        )
    };
    rows.push(field(
        "CreateRoom with a name outside the modelled pattern",
        "CreateRoom",
        json!({ "name": "no spaces allowed" }),
        "name",
        format!("does not match the required pattern {NAME_PATTERN}"),
    ));
    rows.push(field(
        "CreateLoggingConfiguration with a name outside the modelled pattern",
        "CreateLoggingConfiguration",
        json!({
            "name": "no spaces allowed",
            "destinationConfiguration": { "s3": { "bucketName": "chat-logs" } },
        }),
        "name",
        format!("does not match the required pattern {NAME_PATTERN}"),
    ));
    rows.push(field(
        "UpdateRoom with a name outside the modelled pattern",
        "UpdateRoom",
        json!({ "identifier": room, "name": "no spaces allowed" }),
        "name",
        format!("does not match the required pattern {NAME_PATTERN}"),
    ));
    rows.push(field(
        "CreateRoom with a name past the modelled length bound",
        "CreateRoom",
        json!({ "name": over_long_name }),
        "name",
        format!(
            "is required to have a maximum length of {NAME_MAX_LENGTH}, found {}",
            NAME_MAX_LENGTH + 1
        ),
    ));
    rows.push(field(
        "ListRooms with a page size above the modelled ceiling",
        "ListRooms",
        json!({ "maxResults": 100 }),
        "maxResults",
        "is required to have a maximum value of 50.0, found 100.0".to_string(),
    ));
    rows.push(field(
        "ListRooms with a page size below the modelled floor",
        "ListRooms",
        json!({ "maxResults": 0 }),
        "maxResults",
        "is required to have a minimum value of 1.0, found 0.0".to_string(),
    ));
    rows.push(field(
        "CreateChatToken with a session duration above the modelled ceiling",
        "CreateChatToken",
        json!({ "roomIdentifier": room, "userId": "u", "sessionDurationInMinutes": 181 }),
        "sessionDurationInMinutes",
        "is required to have a maximum value of 180.0, found 181.0".to_string(),
    ));
    rows.push(field(
        "CreateRoom with a message length above the modelled ceiling",
        "CreateRoom",
        json!({ "maximumMessageLength": 501 }),
        "maximumMessageLength",
        "is required to have a maximum value of 500.0, found 501.0".to_string(),
    ));
    rows.push(field(
        "CreateChatToken without the required userId",
        "CreateChatToken",
        json!({ "roomIdentifier": room }),
        "userId",
        "is required".to_string(),
    ));
    rows
}

/// RULE: every refusal carries the whole error document, the discriminator, the modelled
/// members and the id of the request it answers.
#[tokio::test]
async fn every_refusal_carries_the_error_document_a_client_reads_a_failure_out_of() {
    let emulator = Emulator::new();
    for (case, call, expected) in rows() {
        let answer = emulator
            .call_raw(call.method, &call.uri, &call.body)
            .await;
        assert_envelope(&case, &answer, &expected);
    }
}

/// RULE: the id names ONE response — two refusals of the same call are never reported under
/// one request id.
#[tokio::test]
async fn two_refusals_name_two_requests() {
    let emulator = Emulator::new();
    let room = absent_room();
    let expected = Expected {
        status: 404,
        code: "ResourceNotFoundException",
        message: not_found_message(&room),
        details: not_found_details(&room, "ROOM"),
    };
    let call = rpc("GetRoom", &json!({ "identifier": room }));
    let mut seen: Vec<String> = Vec::new();
    for attempt in 0..4 {
        let answer = emulator
            .call_raw(call.method, &call.uri, &call.body)
            .await;
        let case = format!("GetRoom refusal {attempt}");
        let request_id = assert_envelope(&case, &answer, &expected);
        assert!(
            !seen.contains(&request_id),
            "case {case:?}: expected a fresh request id, got the already-stated \
             {request_id:?}"
        );
        seen.push(request_id);
    }
}

/// RULE, the other half: a SUCCESS names the request it answered too, and carries none of
/// the error document's members — the envelope belongs to a failure.
#[tokio::test]
async fn a_success_names_its_request_and_carries_no_error_document() {
    let emulator = Emulator::new();
    // Rows differ in the operation and in whether its answer carries a document at all:
    // a create, a read, a listing, and the empty-bodied 204 of a removal.
    let created = emulator.create_room(&json!({ "name": "named" })).await;
    let arn = created
        .get("arn")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(!arn.is_empty(), "CreateRoom must answer an ARN, got {created}");

    let rows: Vec<(&str, Call)> = vec![
        ("GetRoom", rpc("GetRoom", &json!({ "identifier": arn }))),
        ("ListRooms", rpc("ListRooms", &json!({}))),
        ("ListTagsForResource", list_tags(&arn)),
        ("DeleteRoom", rpc("DeleteRoom", &json!({ "identifier": arn }))),
    ];
    for (case, call) in rows {
        let answer = emulator
            .call_raw(call.method, &call.uri, &call.body)
            .await;
        assert!(
            answer.status < 400,
            "case {case:?}: expected a success, got {} {:?}",
            answer.status,
            answer.text()
        );
        assert!(
            answer
                .header(REQUEST_ID_HEADER)
                .is_some_and(|id| !id.is_empty()),
            "case {case:?}: a success names the request it answers too, got {:?}",
            answer.headers
        );
        if answer.body.is_empty() {
            continue;
        }
        let document = document(&answer);
        for member in [CODE, MESSAGE, DETAILS, REQUEST_ID, TYPE_KEY] {
            assert!(
                document.get(member).is_none(),
                "case {case:?}: a success must carry no {member:?}, got {document:?}"
            );
        }
    }
}
