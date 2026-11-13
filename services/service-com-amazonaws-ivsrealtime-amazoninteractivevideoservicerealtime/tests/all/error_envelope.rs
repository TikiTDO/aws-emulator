// SPDX-License-Identifier: CC0-1.0

//! The envelope every IVS RealTime response carries, driven end-to-end over the router
//! the production entrypoint serves.
//!
//! One rule, three readings of it:
//!
//! 1. a refused call answers a JSON error document stating the modelled error shape's
//!    name under `Code`, the failure's text under `Message` and the id of the request it
//!    answers under `RequestID`, with the same code in `x-amzn-ErrorType`, the same text
//!    in `x-amzn-ErrorMessage`, the same id in `x-amzn-RequestId`, and the text also under
//!    the `message` key an SDK error parser reads;
//! 2. the text of a not-found refusal is built from the request by one rule —
//!    `Resource: <the identifier the caller sent> not found` — in every operation that
//!    takes an identifier;
//! 3. the text of a constraint refusal is built from the request by one rule — the
//!    OPERATION, the dotted member path and the bound it violated, with what the request
//!    sent — for every constraint class the model declares, while a value ON a published
//!    bound is accepted and honoured.
//!
//! Every response — a success included — names the request it answers, and no two
//! responses name the same one.
//!
//! Each table's rows differ along what the rule generalises over (the operation, the error
//! class, the member, the constraint class, the bound, the identifier), never by renaming
//! one case.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The account every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// The region this suite signs for.
const REGION: &str = "us-west-2";

/// The three members of the error document a client of this service reads a failure out
/// of, and the three response headers that carry the same three facts.
const CODE_KEY: &str = "Code";
const MESSAGE_KEY: &str = "Message";
const REQUEST_ID_KEY: &str = "RequestID";
const ERROR_TYPE_HEADER: &str = "x-amzn-errortype";
const ERROR_MESSAGE_HEADER: &str = "x-amzn-errormessage";
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// One response, as a client sees it.
struct Answer {
    status: u16,
    headers: BTreeMap<String, String>,
    body: Value,
}

impl Answer {
    /// A body member as text, or `"<absent>"` so a failing assertion says which member
    /// was missing rather than panicking inside the reader.
    fn member(&self, key: &str) -> &str {
        self.body.get(key).and_then(Value::as_str).unwrap_or("<absent>")
    }

    /// A response header, or `"<absent>"`.
    fn header(&self, name: &str) -> &str {
        self.headers.get(name).map_or("<absent>", String::as_str)
    }
}

/// One emulator holding only this service, driven through the router that serves
/// production traffic.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Pin authentication off so the suite does not depend on the process setting.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService::new(
                engine,
            ),
        );
        Self { router }
    }

    /// One request over the wire, signed with a credential scope naming `ivs` in
    /// [`REGION`] as an SDK would.
    async fn call(&self, method: &str, uri: &str, body: &str) -> Answer {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/ivs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00"
        );
        let request = http::Request::builder()
            .method(method)
            .uri(uri.to_string())
            .header("authorization", authorization)
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(body.to_owned()))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let headers: BTreeMap<String, String> = response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_ascii_lowercase(),
                    String::from_utf8_lossy(value.as_bytes()).to_string(),
                )
            })
            .collect();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).to_string()))
        };
        Answer {
            status,
            headers,
            body,
        }
    }

    /// One of the operations the model binds to `POST /<Operation>`.
    async fn post(&self, operation: &str, body: &Value) -> Answer {
        self.call("POST", &format!("/{operation}"), &body.to_string())
            .await
    }

    /// Create a stage and return the ARN the service minted for it.
    async fn create_stage(&self, body: &Value) -> String {
        let answer = self.post("CreateStage", body).await;
        assert_eq!(
            answer.status, 200,
            "CreateStage {body}: expected 200, got {} {}",
            answer.status, answer.body
        );
        answer.body["stage"]["arn"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }
}

/// A well-formed stage ARN naming a stage that was never created.
fn absent_stage_arn(id: &str) -> String {
    format!("arn:aws:ivs:{REGION}:{ACCOUNT}:stage/{id}")
}

/// A well-formed public-key ARN, for the rows that call an operation this emulator has
/// not implemented: the ARN satisfies the modelled `PublicKeyArn` pattern, so the request
/// reaches the handler rather than the boundary validator.
fn public_key_arn(id: &str) -> String {
    format!("arn:aws:ivs:{REGION}:{ACCOUNT}:public-key/{id}")
}

/// The one message rule every not-found refusal of this service follows.
fn not_found_message(identifier: &str) -> String {
    format!("Resource: {identifier} not found")
}

/// The message rule a `@range` refusal follows: the operation, the member, the violated
/// bound and the submitted value, both numbers in decimal form.
fn range_message(operation: &str, member: &str, kind: &str, bound: i64, found: i64) -> String {
    format!("{operation}.{member} is required to have a {kind} value of {bound}.0, found {found}.0")
}

/// The message rule a `@pattern` refusal follows.
fn pattern_message(operation: &str, member: &str, pattern: &str) -> String {
    format!("{operation}.{member} does not match the required pattern {pattern}")
}

/// The message rule a `@length` refusal follows.
fn length_message(operation: &str, member: &str, kind: &str, bound: i64, found: usize) -> String {
    format!("{operation}.{member} is required to have a {kind} length of {bound}, found {found}")
}

/// Whether `id` takes the 8-4-4-4-12 lowercase-hex form an AWS request id takes.
fn is_request_id_shaped(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.iter().map(|group| group.len()).collect::<Vec<usize>>() == vec![8, 4, 4, 4, 12]
        && groups.iter().all(|group| {
            group
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        })
}

/// Assert the whole envelope of ONE refusal: the document's three members, the three
/// headers carrying the same three facts, and the `message` key an SDK reads.
///
/// `expected_message` is `None` for a failure whose text no rule of this service fixes,
/// where the requirement is only that a text is stated.
fn assert_stated_failure(
    case: &str,
    answer: &Answer,
    expected_status: u16,
    expected_code: &str,
    expected_message: Option<&str>,
) {
    assert_eq!(
        answer.status, expected_status,
        "case {case:?}: expected HTTP {expected_status}, got {} {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.member(CODE_KEY),
        expected_code,
        "case {case:?}: expected the document's {CODE_KEY} to be {expected_code:?}, got {}",
        answer.body
    );
    match expected_message {
        Some(expected) => assert_eq!(
            answer.member(MESSAGE_KEY),
            expected,
            "case {case:?}: expected the document's {MESSAGE_KEY} to follow the service's \
             message rule, got {}",
            answer.body
        ),
        None => assert!(
            !answer.member(MESSAGE_KEY).is_empty()
                && answer.member(MESSAGE_KEY) != "<absent>",
            "case {case:?}: expected a stated {MESSAGE_KEY}, got {}",
            answer.body
        ),
    }
    // The text an SDK error parser reads is the same text, not a second rendering of it.
    assert_eq!(
        answer.member("message"),
        answer.member(MESSAGE_KEY),
        "case {case:?}: expected the SDK `message` key to carry the stated text, got {}",
        answer.body
    );
    let request_id = answer.member(REQUEST_ID_KEY);
    assert!(
        is_request_id_shaped(request_id),
        "case {case:?}: expected the document's {REQUEST_ID_KEY} to take the 8-4-4-4-12 \
         lowercase-hex form, got {request_id:?} in {}",
        answer.body
    );
    assert_eq!(
        answer.header(ERROR_TYPE_HEADER),
        expected_code,
        "case {case:?}: expected {ERROR_TYPE_HEADER} to name the modelled error shape, got \
         {:?}",
        answer.headers
    );
    assert_eq!(
        answer.header(ERROR_MESSAGE_HEADER),
        answer.member(MESSAGE_KEY),
        "case {case:?}: expected {ERROR_MESSAGE_HEADER} to carry the stated text, got {:?}",
        answer.headers
    );
    assert_eq!(
        answer.header(REQUEST_ID_HEADER),
        request_id,
        "case {case:?}: expected {REQUEST_ID_HEADER} to name the same request as the \
         document, got {:?}",
        answer.headers
    );
}

/// Every class of failure this service answers states its code, its text and the request
/// it answers — one row per (operation, error class) pair the service can reach.
#[tokio::test]
async fn every_refused_call_states_its_code_its_text_and_the_request_it_answers() {
    let emulator = Emulator::new();
    let live = emulator.create_stage(&json!({ "name": "envelope" })).await;
    let absent = absent_stage_arn("neverCreated");

    // Rows span the error CLASSES this service answers (404 from a handler, 400 from the
    // constraint walk, 400 from a handler's own check) across the operations that answer
    // them.
    let cases: Vec<RefusalCase> = vec![
        RefusalCase {
            case: "a token for a stage that does not exist",
            operation: "CreateParticipantToken",
            request: json!({ "stageArn": absent }),
            status: 404,
            code: "ResourceNotFoundException",
            message: Some(not_found_message(&absent)),
        },
        RefusalCase {
            case: "a read of a stage that does not exist",
            operation: "GetStage",
            request: json!({ "arn": absent }),
            status: 404,
            code: "ResourceNotFoundException",
            message: Some(not_found_message(&absent)),
        },
        RefusalCase {
            case: "a duration above the modelled maximum",
            operation: "CreateParticipantToken",
            request: json!({ "stageArn": live, "duration": 20_161 }),
            status: 400,
            code: "ValidationException",
            message: Some(range_message(
                "CreateParticipantToken",
                "duration",
                "maximum",
                20_160,
                20_161,
            )),
        },
        RefusalCase {
            case: "a stage name outside the modelled pattern",
            operation: "CreateStage",
            request: json!({ "name": "not a legal name!" }),
            status: 400,
            code: "ValidationException",
            message: Some(pattern_message("CreateStage", "name", "^[a-zA-Z0-9-_]*$")),
        },
        RefusalCase {
            case: "a required member the request omits",
            operation: "CreateParticipantToken",
            request: json!({ "duration": 60 }),
            status: 400,
            code: "ValidationException",
            message: Some("CreateParticipantToken.stageArn is required".to_string()),
        },
        RefusalCase {
            case: "a pagination token this service never issued",
            operation: "ListStages",
            request: json!({ "nextToken": "bm90LWEtdG9rZW4=" }),
            status: 400,
            code: "ValidationException",
            message: Some(
                "ListStages.nextToken is required to be a pagination token issued by ListStages"
                    .to_string(),
            ),
        },
        RefusalCase {
            case: "a delete naming a public key this account holds none of",
            operation: "DeletePublicKey",
            request: json!({ "arn": public_key_arn("neverImported") }),
            status: 404,
            code: "ResourceNotFoundException",
            message: Some(not_found_message(&public_key_arn("neverImported"))),
        },
    ];

    for row in cases {
        let answer = emulator.post(row.operation, &row.request).await;
        assert_stated_failure(
            row.case,
            &answer,
            row.status,
            row.code,
            row.message.as_deref(),
        );
    }
}

/// One row of the refusal table: the call, and the envelope it must answer with.
struct RefusalCase {
    case: &'static str,
    operation: &'static str,
    request: Value,
    status: u16,
    code: &'static str,
    /// `None` for a failure whose text no rule of this service fixes.
    message: Option<String>,
}

/// The not-found rule: one sentence, built from the identifier the caller sent, in every
/// operation that takes one.
#[tokio::test]
async fn a_not_found_refusal_names_the_identifier_the_caller_sent() {
    let emulator = Emulator::new();
    // A real stage exists throughout, so each 404 is about the identifier the request
    // carried and not about an empty store.
    let live = emulator.create_stage(&json!({ "name": "live" })).await;

    // A stage that WAS created and then deleted: the same rule has to cover an identifier
    // that was valid a moment ago.
    let doomed = emulator.create_stage(&json!({ "name": "doomed" })).await;
    let deleted = emulator.post("DeleteStage", &json!({ "arn": doomed })).await;
    assert_eq!(
        deleted.status, 200,
        "DeleteStage must answer its modelled 200, got {} {}",
        deleted.status, deleted.body
    );

    // Rows differ in HOW the identifier fails to name a stage: never created, created and
    // then deleted, a different id form, and a well-formed ARN in another region.
    let identifiers = vec![
        ("never created", absent_stage_arn("neverCreated1")),
        ("created then deleted", doomed.clone()),
        ("a twelve-character id", absent_stage_arn("abc123ABC456")),
        ("an id carrying a dash", absent_stage_arn("stage-7")),
        (
            "another region",
            format!("arn:aws:ivs:eu-west-2:{ACCOUNT}:stage/elsewhere01"),
        ),
    ];

    for (case, identifier) in &identifiers {
        let calls: Vec<(&str, Value)> = vec![
            ("GetStage", json!({ "arn": identifier })),
            (
                "UpdateStage",
                json!({ "arn": identifier, "name": "renamed" }),
            ),
            ("DeleteStage", json!({ "arn": identifier })),
            (
                "CreateParticipantToken",
                json!({ "stageArn": identifier, "userId": "u" }),
            ),
        ];
        for (operation, request) in calls {
            let answer = emulator.post(operation, &request).await;
            assert_stated_failure(
                &format!("{case} / {operation}"),
                &answer,
                404,
                "ResourceNotFoundException",
                Some(&not_found_message(identifier)),
            );
        }
    }

    // The stage that does exist is untouched by all of that: the refusals were about the
    // identifier.
    let answer = emulator.post("GetStage", &json!({ "arn": live })).await;
    assert_eq!(
        answer.status, 200,
        "the stage that does exist still reads back, got {} {}",
        answer.status, answer.body
    );
}

/// The constraint rule: the operation, the dotted member path and the bound it violated,
/// with what the request sent — one row per constraint class the model declares, on each
/// published bound and one past it.
#[tokio::test]
async fn a_value_outside_a_modelled_constraint_is_refused_with_the_services_rule() {
    let emulator = Emulator::new();
    let stage = emulator.create_stage(&json!({ "name": "constraints" })).await;

    // Bounds from `smithy_query(query="operation",
    // operation="CreateParticipantToken,CreateStage,ListStages")`:
    // CreateParticipantToken.duration @range 1..20160, .userId @length 0..128,
    // .capabilities @length 0..2, .stageArn @pattern ^arn:aws:ivs:…:stage/[a-zA-Z0-9-]+$;
    // CreateStage.name @pattern ^[a-zA-Z0-9-_]*$, .participantTokenConfigurations
    // @length 0..12; ListStages.maxResults @range 1..100, .nextToken @pattern
    // ^[a-zA-Z0-9+/=_-]*$.
    let over_long_user_id = "u".repeat(129);
    let too_many_capabilities = json!(["PUBLISH", "SUBSCRIBE", "PUBLISH"]);
    let thirteen_configurations: Vec<Value> = (0..13).map(|_| json!({ "userId": "u" })).collect();

    let refused: Vec<(&str, &str, Value, String)> = vec![
        (
            "duration one above the maximum",
            "CreateParticipantToken",
            json!({ "stageArn": stage, "duration": 20_161 }),
            range_message(
                "CreateParticipantToken",
                "duration",
                "maximum",
                20_160,
                20_161,
            ),
        ),
        (
            "duration one below the minimum",
            "CreateParticipantToken",
            json!({ "stageArn": stage, "duration": 0 }),
            range_message("CreateParticipantToken", "duration", "minimum", 1, 0),
        ),
        (
            "duration far below the minimum",
            "CreateParticipantToken",
            json!({ "stageArn": stage, "duration": -7 }),
            range_message("CreateParticipantToken", "duration", "minimum", 1, -7),
        ),
        (
            "maxResults one above the maximum",
            "ListStages",
            json!({ "maxResults": 101 }),
            range_message("ListStages", "maxResults", "maximum", 100, 101),
        ),
        (
            "maxResults one below the minimum",
            "ListStages",
            json!({ "maxResults": 0 }),
            range_message("ListStages", "maxResults", "minimum", 1, 0),
        ),
        (
            "a userId one character over its length",
            "CreateParticipantToken",
            json!({ "stageArn": stage, "userId": over_long_user_id }),
            length_message("CreateParticipantToken", "userId", "maximum", 128, 129),
        ),
        (
            "one capability too many",
            "CreateParticipantToken",
            json!({ "stageArn": stage, "capabilities": too_many_capabilities }),
            length_message("CreateParticipantToken", "capabilities", "maximum", 2, 3),
        ),
        (
            "one token configuration too many",
            "CreateStage",
            json!({ "participantTokenConfigurations": thirteen_configurations }),
            length_message(
                "CreateStage",
                "participantTokenConfigurations",
                "maximum",
                12,
                13,
            ),
        ),
        (
            "a stageArn outside its pattern",
            "CreateParticipantToken",
            json!({ "stageArn": "not-an-arn" }),
            pattern_message(
                "CreateParticipantToken",
                "stageArn",
                "^arn:aws:ivs:[a-z0-9-]+:[0-9]+:stage/[a-zA-Z0-9-]+$",
            ),
        ),
        (
            "a stage name outside its pattern",
            "CreateStage",
            json!({ "name": "spaces and !" }),
            pattern_message("CreateStage", "name", "^[a-zA-Z0-9-_]*$"),
        ),
        (
            "a pagination token outside its pattern",
            "ListStages",
            json!({ "nextToken": "not a token!" }),
            pattern_message("ListStages", "nextToken", "^[a-zA-Z0-9+/=_-]*$"),
        ),
    ];

    for (case, operation, request, message) in refused {
        let answer = emulator.post(operation, &request).await;
        assert_stated_failure(case, &answer, 400, "ValidationException", Some(&message));
    }

    // A value ON a published bound is accepted and honoured, never refused and never
    // clamped — the other half of the same rule.
    let twelve_configurations: Vec<Value> = (0..12).map(|_| json!({ "userId": "u" })).collect();
    let accepted: Vec<(&str, &str, Value)> = vec![
        (
            "duration on the minimum",
            "CreateParticipantToken",
            json!({ "stageArn": stage, "duration": 1 }),
        ),
        (
            "duration on the maximum",
            "CreateParticipantToken",
            json!({ "stageArn": stage, "duration": 20_160 }),
        ),
        (
            "a userId of exactly its maximum length",
            "CreateParticipantToken",
            json!({ "stageArn": stage, "userId": "u".repeat(128) }),
        ),
        (
            "both capabilities",
            "CreateParticipantToken",
            json!({ "stageArn": stage, "capabilities": ["PUBLISH", "SUBSCRIBE"] }),
        ),
        ("maxResults on the minimum", "ListStages", json!({ "maxResults": 1 })),
        ("maxResults on the maximum", "ListStages", json!({ "maxResults": 100 })),
        (
            "twelve token configurations",
            "CreateStage",
            json!({ "participantTokenConfigurations": twelve_configurations }),
        ),
    ];

    for (case, operation, request) in accepted {
        let answer = emulator.post(operation, &request).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a value on a published bound must be accepted, got {} {}",
            answer.status, answer.body
        );
    }

    // The duration on each bound is the duration the token carries: accepted, not clamped.
    for (case, requested) in [("the minimum", 1_i64), ("the maximum", 20_160_i64)] {
        let answer = emulator
            .post(
                "CreateParticipantToken",
                &json!({ "stageArn": stage, "duration": requested }),
            )
            .await;
        assert_eq!(
            answer.body["participantToken"]["duration"].as_i64(),
            Some(requested),
            "case {case:?}: expected the token to carry the requested duration, got {}",
            answer.body
        );
    }
}

/// Every response names the request it answers — a success as much as a failure — and no
/// two responses name the same one.
#[tokio::test]
async fn every_response_names_the_one_request_it_answers() {
    let emulator = Emulator::new();
    let stage = emulator.create_stage(&json!({ "name": "ids" })).await;
    let absent = absent_stage_arn("neverCreated2");

    // Rows span every response class: a success with a body, the modelled empty-body
    // success, a handler refusal, a boundary refusal, and a request naming no modelled
    // operation at all.
    let calls: Vec<(&str, &str, Value)> = vec![
        ("a success with a body", "GetStage", json!({ "arn": stage })),
        ("a list success", "ListStages", json!({})),
        (
            "a minted token",
            "CreateParticipantToken",
            json!({ "stageArn": stage, "userId": "u" }),
        ),
        (
            "a handler refusal",
            "GetStage",
            json!({ "arn": absent }),
        ),
        (
            "a boundary refusal",
            "CreateParticipantToken",
            json!({ "stageArn": stage, "duration": 20_161 }),
        ),
        (
            "a handler refusal from a delete",
            "DeletePublicKey",
            json!({ "arn": public_key_arn("neverImported3") }),
        ),
    ];

    let mut seen: Vec<String> = Vec::new();
    for (case, operation, request) in calls {
        let answer = emulator.post(operation, &request).await;
        let id = answer.header(REQUEST_ID_HEADER).to_string();
        assert!(
            is_request_id_shaped(&id),
            "case {case:?}: expected {REQUEST_ID_HEADER} to take the 8-4-4-4-12 \
             lowercase-hex form, got {id:?} in {:?}",
            answer.headers
        );
        assert!(
            !seen.contains(&id),
            "case {case:?}: expected a fresh request id, got the already-answered {id:?}"
        );
        seen.push(id);
    }

    // The modelled empty-body success and an unroutable request are responses too.
    let deleted = emulator.post("DeleteStage", &json!({ "arn": stage })).await;
    assert_eq!(
        deleted.status, 200,
        "DeleteStage must answer its modelled 200, got {} {}",
        deleted.status, deleted.body
    );
    let id = deleted.header(REQUEST_ID_HEADER).to_string();
    assert!(
        is_request_id_shaped(&id),
        "case \"an empty-body success\": expected a shaped request id, got {id:?} in {:?}",
        deleted.headers
    );
    assert!(
        !seen.contains(&id),
        "case \"an empty-body success\": expected a fresh request id, got {id:?}"
    );

    // A request this service claims but that names no modelled operation — the same path
    // with a method the model does not bind — is answered by the service's own routing
    // error, which names the request it answers too.
    let unroutable = emulator.call("GET", "/GetStage", "").await;
    assert!(
        unroutable.status >= 400,
        "case \"a method the model does not bind\": expected a failure, got {} {}",
        unroutable.status, unroutable.body
    );
    let id = unroutable.header(REQUEST_ID_HEADER).to_string();
    assert!(
        is_request_id_shaped(&id),
        "case \"a method the model does not bind\": expected a shaped request id, got \
         {id:?} in {:?}",
        unroutable.headers
    );
}
