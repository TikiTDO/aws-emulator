// SPDX-License-Identifier: CC0-1.0

//! Which refusal `ListRealtimeContactAnalysisSegments` answers with, and in which order the
//! stages ahead of its handler decide.
//!
//! THE RULES UNDER TEST, each driven end to end over the real wire path (a restJson1 request
//! through the `MultiServiceRouter` into the mounted service), so what is asserted is what a
//! client sees:
//!
//! 1. the `InstanceId` a request names is resolved to an Amazon Connect instance BEFORE the
//!    body's own `@length` / `@pattern` clauses are applied to it; an identifier that is not
//!    one an Amazon Connect instance is addressed by (a GUID, bare or as the resource of that
//!    instance's ARN — the form Amazon Connect's own model declares for `InstanceId`) resolves
//!    to no instance and the call is refused `UnauthorizedException` with the fixed text
//!    `Unauthorized`;
//! 2. identity-based IAM policy is evaluated next, independently of whether the contact has a
//!    session, so a caller the policies do not allow gets the modelled `AccessDeniedException`
//!    (HTTP 403) and a privileged caller issuing the identical request reaches the lookup;
//! 3. a request that fails a MODELLED input constraint is refused with the operation's own
//!    `InvalidRequestException` (HTTP 400) carrying the body validator's finding list —
//!    `Invalid request body: [<finding>, <finding>]` — and never a `ValidationException`,
//!    which this service's model does not declare.
//!
//! The tables span what each rule generalises over: the form, case and length of the
//! identifier; which member breaks which clause class; whether the request carries
//! `MaxResults` and which value; and which policy is in force on which kind of principal.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_connectcontactlens_amazonconnectcontactlens as contact_lens;

use contact_lens::state::{service_state, AnalysisSession};

/// The operation's modelled route (`smithy_query(query="operation")`: `POST
/// /realtime-contact-analysis/analysis-segments`).
const URI: &str = "/realtime-contact-analysis/analysis-segments";

/// The Region every call here is signed for, which is the Region the judged resource's ARN
/// carries.
const REGION: &str = "us-east-1";

/// The IAM action namespace Contact Lens publishes its actions under: this service's
/// `arnNamespace`, which is also its `sigv4Name` (`smithy_query(query="service_info")`).
const ACTION_PREFIX: &str = "connect";

/// The action this operation is judged by.
const ACTION: &str = "connect:ListRealtimeContactAnalysisSegments";

/// The access key id of the account's own published credential: a key neither STS nor IAM
/// holds a record for, which is the position AWS account root credentials are in.
const ACCOUNT_KEY: &str = "AKIAIOSFODNN7EXAMPLE";

/// A GUID-shaped Amazon Connect instance identifier, and a contact of it.
const INSTANCE: &str = "4cd1aa0d-1f2e-4b3c-8d4e-5f60718293a4";
const CONTACT: &str = "b5f87632-2a3b-4c5d-9e8f-0a1b2c3d4e5f";

/// The fixed sentence the lookup stage refuses a contact with no stored session with.
const NOT_FOUND_MESSAGE: &str = "Real-time contact analysis not found.";

/// What one call answered: its status, the restJson1 error discriminator and the parsed body.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: String,
    body: Value,
}

impl Reply {
    /// The message the answer carries, under either spelling: a modelled error of this
    /// service declares `Message`, while the front door's own refusal — a shape no model
    /// declares — carries `message`.
    fn message(&self) -> String {
        self.body
            .get("Message")
            .or_else(|| self.body.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// The emulator these tests drive: one shared engine carrying this service beside the IAM and
/// STS services the principals and their policies are made with.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing: the `Authorization` header carries only the credential
        // scope, which is where a service reads the caller's access key id from.
        router.set_request_authentication(false);
        router.register_service(contact_lens::EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router, engine }
    }

    /// The seeding path: register an analysis session on the shared engine the way the
    /// producing side (Amazon Connect's voice analysis) would.
    fn seed(&self, instance: &str, contact: &str) {
        service_state(&*self.engine)
            .expect("the in-memory engine must expose this service's state")
            .lock()
            .put_session(AnalysisSession::new(instance, contact).completed(true));
    }

    /// One call to this operation, with `access_key_id` in the SigV4 credential scope.
    async fn list(&self, access_key_id: &str, body: &Value) -> Reply {
        let request = http::Request::builder()
            .method("POST")
            .uri(URI)
            .header("host", format!("contact-lens.{REGION}.amazonaws.com"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/\
                     {ACTION_PREFIX}/aws4_request, SignedHeaders=host, Signature=0"
                ),
            )
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_code,
            body: parsed,
        }
    }

    /// One awsQuery call to IAM or STS, signed with the account's own credential.
    async fn query(
        &self,
        signing_name: &str,
        version: &str,
        action: &str,
        params: &[(&str, String)],
    ) -> String {
        let mut body = format!("Action={action}&Version={version}");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{signing_name}.amazonaws.com/"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/\
                     {signing_name}/aws4_request, SignedHeaders=host, Signature=0"
                ),
            )
            .body(Bytes::from(body))
            .expect("a well-formed awsQuery request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let text = String::from_utf8_lossy(response.body()).to_string();
        assert_eq!(
            status, 200,
            "case {action:?}: the setup call must succeed, got {status} with {text}"
        );
        text
    }

    /// Create an IAM user carrying `policy` as its inline policy (when one is given), mint a
    /// long-term access key for it, and answer that key id with the user's own ARN.
    async fn user(&self, user_name: &str, policy: Option<&str>) -> Principal {
        let created = self
            .query(
                "iam",
                "2010-05-08",
                "CreateUser",
                &[("UserName", user_name.to_string())],
            )
            .await;
        let arn = xml_element(&created, "Arn")
            .unwrap_or_else(|| panic!("CreateUser answers the user's Arn, got {created}"));
        if let Some(policy) = policy {
            self.query(
                "iam",
                "2010-05-08",
                "PutUserPolicy",
                &[
                    ("UserName", user_name.to_string()),
                    ("PolicyName", "inline".to_string()),
                    ("PolicyDocument", policy.to_string()),
                ],
            )
            .await;
        }
        let minted = self
            .query(
                "iam",
                "2010-05-08",
                "CreateAccessKey",
                &[("UserName", user_name.to_string())],
            )
            .await;
        Principal {
            access_key_id: xml_element(&minted, "AccessKeyId")
                .unwrap_or_else(|| panic!("CreateAccessKey answers an AccessKeyId, got {minted}")),
            arn,
        }
    }

    /// Create an IAM role carrying `policy` as its inline policy (when one is given), assume
    /// it through STS, and answer the session's access key id with the session ARN.
    async fn assumed_role(&self, role_name: &str, policy: Option<&str>) -> Principal {
        let trust = format!(
            "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
             \"Principal\":{{\"AWS\":\"arn:aws:iam::{}:root\"}},\
             \"Action\":\"sts:AssumeRole\"}}]}}",
            ::engine::emulator_account_id()
        );
        self.query(
            "iam",
            "2010-05-08",
            "CreateRole",
            &[
                ("RoleName", role_name.to_string()),
                ("AssumeRolePolicyDocument", trust),
            ],
        )
        .await;
        if let Some(policy) = policy {
            self.query(
                "iam",
                "2010-05-08",
                "PutRolePolicy",
                &[
                    ("RoleName", role_name.to_string()),
                    ("PolicyName", "inline".to_string()),
                    ("PolicyDocument", policy.to_string()),
                ],
            )
            .await;
        }
        let assumed = self
            .query(
                "sts",
                "2011-06-15",
                "AssumeRole",
                &[
                    (
                        "RoleArn",
                        format!(
                            "arn:aws:iam::{}:role/{role_name}",
                            ::engine::emulator_account_id()
                        ),
                    ),
                    ("RoleSessionName", "probe".to_string()),
                ],
            )
            .await;
        Principal {
            access_key_id: xml_element(&assumed, "AccessKeyId")
                .unwrap_or_else(|| panic!("AssumeRole answers an AccessKeyId, got {assumed}")),
            arn: xml_element(&assumed, "Arn")
                .unwrap_or_else(|| panic!("AssumeRole answers the session's Arn, got {assumed}")),
        }
    }
}

/// A principal a call is signed as: the access key id it presents, and the ARN a refusal must
/// blame it by — what `sts:GetCallerIdentity` reports for that key.
struct Principal {
    access_key_id: String,
    arn: String,
}

/// Percent-encode one `application/x-www-form-urlencoded` component, leaving only RFC 3986's
/// unreserved characters literal — so a policy document's braces and quotes survive the round
/// trip.
fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// The text of the first `<name>…</name>` element of an awsQuery response.
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// The request body, with only the members it is given.
fn request(instance: &str, contact: Option<&str>, max_results: Option<i32>) -> Value {
    let mut body = json!({ "InstanceId": instance });
    if let Some(contact) = contact {
        body["ContactId"] = json!(contact);
    }
    if let Some(max_results) = max_results {
        body["MaxResults"] = json!(max_results);
    }
    body
}

/// The ARN the action is judged against: the contact the request addresses, under the instance
/// it names.
fn judged_resource(instance: &str, contact: &str) -> String {
    ::engine::emulator_arn(
        ACTION_PREFIX,
        REGION,
        &format!("instance/{instance}/contact/{contact}"),
    )
}

/// A policy document allowing every action of this service, and explicitly denying `denied`.
fn allow_all_but(denied: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[\
         {{\"Effect\":\"Allow\",\"Action\":\"{ACTION_PREFIX}:*\",\"Resource\":\"*\"}},\
         {{\"Effect\":\"Deny\",\"Action\":\"{denied}\",\"Resource\":\"*\"}}]}}"
    )
}

/// A policy document allowing `action` on every resource.
fn allow(action: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Action\":\"{action}\",\"Resource\":\"*\"}}]}}"
    )
}

/// Assert the answer carries no `Segments` key: a refusal reports nothing of the contact.
fn assert_no_segments(case: &str, reply: &Reply) {
    assert!(
        reply.body.get("Segments").is_none(),
        "case {case:?}: a refusal must carry no Segments key, got {:?}",
        reply.body
    );
}

// ---------------------------------------------------------------------------
// Stage 1 — resolving the InstanceId
// ---------------------------------------------------------------------------

/// RULE: an `InstanceId` that is not an Amazon Connect instance identifier resolves to no
/// instance and the call is refused `UnauthorizedException` / "Unauthorized" — before the
/// body's own clauses are applied to it, and whatever the request's other members say.
#[tokio::test]
async fn an_unresolvable_instance_id_is_unauthorized() {
    let emulator = Emulator::new();
    let padded = format!("{INSTANCE}{}", "a".repeat(256));
    let rows: [(&str, String); 10] = [
        ("whitespace only", "\t\t".to_string()),
        ("spaces only", "   ".to_string()),
        ("a single space", " ".to_string()),
        ("a non-guid name", "inst-1".to_string()),
        (
            "exactly 256 characters and not a guid",
            padded[..256].to_string(),
        ),
        ("an uppercase guid", INSTANCE.to_uppercase()),
        ("a guid with a group too short", INSTANCE[1..].to_string()),
        (
            "another service's instance arn",
            format!("arn:aws:connectcases:{REGION}:000000000000:instance/{INSTANCE}"),
        ),
        (
            "an arn naming another resource type",
            format!("arn:aws:connect:{REGION}:000000000000:queue/{INSTANCE}"),
        ),
        ("a single character", "i".to_string()),
    ];

    for (case, instance) in rows {
        let reply = emulator
            .list(ACCOUNT_KEY, &request(&instance, Some(CONTACT), None))
            .await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (401, "UnauthorizedException"),
            "case {case:?}: expected 401 UnauthorizedException, got {} {:?} with {:?}",
            reply.status,
            reply.error_code,
            reply.body
        );
        assert_eq!(
            reply.message(),
            "Unauthorized",
            "case {case:?}: expected the fixed text, got {:?}",
            reply.message()
        );
        assert_no_segments(case, &reply);
    }
}

/// RULE: an `InstanceId` that IS an instance identifier clears that stage and is answered by
/// the stage behind it — the session lookup — whether or not the account ever created that
/// instance. The rows span the two forms the identifier takes.
#[tokio::test]
async fn a_resolvable_instance_id_reaches_the_lookup() {
    let emulator = Emulator::new();
    emulator.seed(INSTANCE, CONTACT);
    let unknown = "7d9e1c3a-4b5c-4d6e-8f90-1a2b3c4d5e6f";
    // (case, the instance named, the contact named, the status the lookup answers)
    let rows: [(&str, String, &str, u16); 4] = [
        ("the seeded pair", INSTANCE.to_string(), CONTACT, 200),
        (
            "a stored instance with no session for the contact",
            INSTANCE.to_string(),
            "no-such-contact",
            404,
        ),
        (
            "a guid the account never created",
            unknown.to_string(),
            CONTACT,
            404,
        ),
        (
            "the instance's own arn",
            format!("arn:aws:connect:{REGION}:000000000000:instance/{unknown}"),
            CONTACT,
            404,
        ),
    ];

    for (case, instance, contact, expected) in rows {
        let reply = emulator
            .list(ACCOUNT_KEY, &request(&instance, Some(contact), None))
            .await;
        assert_eq!(
            reply.status, expected,
            "case {case:?}: expected HTTP {expected}, got {} {:?} with {:?}",
            reply.status, reply.error_code, reply.body
        );
        if expected == 404 {
            assert_eq!(
                (reply.error_code.as_str(), reply.message().as_str()),
                ("ResourceNotFoundException", NOT_FOUND_MESSAGE),
                "case {case:?}: expected the lookup's own refusal, got {:?} {:?}",
                reply.error_code,
                reply.message()
            );
        }
    }
}

/// RULE: the instance stage decides BEFORE the body is judged, so a request whose
/// `InstanceId` and `ContactId` are both unusable collapses to exactly one refusal — the
/// instance's.
#[tokio::test]
async fn the_instance_stage_answers_before_the_body_is_judged() {
    let emulator = Emulator::new();
    // (case, the instance named, the contact named)
    let rows: [(&str, &str, Option<&str>); 4] = [
        ("both members whitespace", "   ", Some("\t\t\t")),
        ("both members one space", " ", Some(" ")),
        ("an unresolvable instance and no contact at all", "\t", None),
        (
            "an unresolvable instance and an over-long contact",
            "nope",
            Some("c"),
        ),
    ];

    for (case, instance, contact) in rows {
        let reply = emulator
            .list(ACCOUNT_KEY, &request(instance, contact, Some(100)))
            .await;
        assert_eq!(
            (reply.status, reply.error_code.as_str(), reply.message()),
            (401, "UnauthorizedException", "Unauthorized".to_string()),
            "case {case:?}: expected the one instance-stage refusal, got {} {:?} with {:?}",
            reply.status,
            reply.error_code,
            reply.body
        );
    }
}

/// RULE: the instance stage is about an identifier the request NAMES, so a request that
/// names none has nothing to resolve and is answered by the body stage: the absent
/// `@required` member, stated as the modelled `InvalidRequestException`.
#[tokio::test]
async fn naming_no_instance_at_all_is_the_body_stages_refusal() {
    let emulator = Emulator::new();
    emulator.seed(INSTANCE, CONTACT);
    // (case, the request body, the whole message expected)
    let rows: [(&str, Value, String); 2] = [
        (
            "no instance id at all",
            json!({ "ContactId": CONTACT }),
            r#"Invalid request body: [object has missing required properties (["InstanceId"])]"#
                .to_string(),
        ),
        (
            // The deserializer stops at the first required member it does not find, so the
            // list states that one; the rest are the caller's next answer.
            "neither identifier at all",
            json!({}),
            r#"Invalid request body: [object has missing required properties (["InstanceId"])]"#
                .to_string(),
        ),
    ];

    for (case, body, expected) in rows {
        let reply = emulator.list(ACCOUNT_KEY, &body).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (400, "InvalidRequestException"),
            "case {case:?}: expected 400 InvalidRequestException, got {} {:?} with {:?}",
            reply.status,
            reply.error_code,
            reply.body
        );
        assert_eq!(
            reply.message(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            reply.message()
        );
        assert_no_segments(case, &reply);
    }
}

// ---------------------------------------------------------------------------
// Stage 3 — the modelled input constraints
// ---------------------------------------------------------------------------

/// RULE: a request that breaks a MODELLED input constraint is refused with the operation's own
/// `InvalidRequestException` on HTTP 400, carrying the body validator's finding list inside
/// the fixed `Invalid request body: [...]` text — one finding per broken clause, plus one for
/// each `int32` member the request carries, in the order the input shape declares its members.
///
/// The rows span the clause classes a member of this input can break (pattern, length
/// minimum/maximum, absent required member, range minimum/maximum), WHICH member breaks it
/// (`ContactId`, `NextToken`, `MaxResults`), and whether the request also carries
/// `MaxResults`.
#[tokio::test]
async fn a_broken_input_constraint_is_the_modelled_invalid_request() {
    let emulator = Emulator::new();
    emulator.seed(INSTANCE, CONTACT);
    let over_long = "c".repeat(257);
    // (case, the request body, the whole message expected)
    let rows: [(&str, Value, String); 9] = [
        (
            "a whitespace-only contact id",
            request(INSTANCE, Some("     "), None),
            r#"Invalid request body: [ECMA 262 regex ".*\S.*" does not match input string "     "]"#
                .to_string(),
        ),
        (
            "a tab-only contact id",
            request(INSTANCE, Some("\t\t"), None),
            "Invalid request body: [ECMA 262 regex \".*\\S.*\" does not match input string \
             \"\t\t\"]"
                .to_string(),
        ),
        (
            "a one-space contact id",
            request(INSTANCE, Some(" "), None),
            r#"Invalid request body: [ECMA 262 regex ".*\S.*" does not match input string " "]"#
                .to_string(),
        ),
        (
            "a whitespace-only contact id with the largest page size",
            request(INSTANCE, Some("     "), Some(100)),
            concat!(
                r#"Invalid request body: [ECMA 262 regex ".*\S.*" does not match input string "#,
                r#""     ", format attribute "int32" not supported]"#
            )
            .to_string(),
        ),
        (
            "a whitespace-only contact id with the smallest page size",
            request(INSTANCE, Some("   "), Some(1)),
            concat!(
                r#"Invalid request body: [ECMA 262 regex ".*\S.*" does not match input string "#,
                r#""   ", format attribute "int32" not supported]"#
            )
            .to_string(),
        ),
        (
            "no contact id at all",
            request(INSTANCE, None, None),
            r#"Invalid request body: [object has missing required properties (["ContactId"])]"#
                .to_string(),
        ),
        (
            "a contact id past the declared maximum length",
            request(INSTANCE, Some(&over_long), None),
            format!(
                "Invalid request body: [string \"{over_long}\" is too long (length: 257, \
                 maximum allowed: 256)]"
            ),
        ),
        (
            "a page size below the declared minimum",
            request(INSTANCE, Some(CONTACT), Some(0)),
            concat!(
                "Invalid request body: [numeric instance is lower than the required minimum ",
                "(minimum: 1, found: 0), format attribute \"int32\" not supported]"
            )
            .to_string(),
        ),
        (
            "a page size above the declared maximum",
            request(INSTANCE, Some(CONTACT), Some(101)),
            concat!(
                "Invalid request body: [numeric instance is greater than the required maximum ",
                "(maximum: 100, found: 101), format attribute \"int32\" not supported]"
            )
            .to_string(),
        ),
    ];

    for (case, body, expected) in rows {
        let reply = emulator.list(ACCOUNT_KEY, &body).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (400, "InvalidRequestException"),
            "case {case:?}: expected 400 InvalidRequestException, got {} {:?} with {:?}",
            reply.status,
            reply.error_code,
            reply.body
        );
        assert_eq!(
            reply.message(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            reply.message()
        );
        assert_no_segments(case, &reply);
    }
}

/// RULE: a `NextToken` outside its own `@pattern` is refused the same way — the rule is about
/// the modelled constraint, not about one member.
#[tokio::test]
async fn a_broken_next_token_clause_is_stated_the_same_way() {
    let emulator = Emulator::new();
    emulator.seed(INSTANCE, CONTACT);
    let mut body = request(INSTANCE, Some(CONTACT), None);
    body["NextToken"] = json!("  ");
    let reply = emulator.list(ACCOUNT_KEY, &body).await;
    assert_eq!(
        (reply.status, reply.error_code.as_str()),
        (400, "InvalidRequestException"),
        "case \"a whitespace-only next token\": expected 400 InvalidRequestException, got {} \
         {:?} with {:?}",
        reply.status,
        reply.error_code,
        reply.body
    );
    assert_eq!(
        reply.message(),
        r#"Invalid request body: [ECMA 262 regex ".*\S.*" does not match input string "  "]"#,
        "case \"a whitespace-only next token\": got {:?}",
        reply.message()
    );
}

/// RULE: the `int32` finding is a statement about the validator rather than about the value,
/// so it refuses nothing on its own: a request whose every value satisfies the model is served
/// whatever page size it carries.
#[tokio::test]
async fn carrying_a_page_size_alone_refuses_nothing() {
    let emulator = Emulator::new();
    emulator.seed(INSTANCE, CONTACT);
    for max_results in [None, Some(1), Some(50), Some(100)] {
        let reply = emulator
            .list(ACCOUNT_KEY, &request(INSTANCE, Some(CONTACT), max_results))
            .await;
        assert_eq!(
            reply.status, 200,
            "case MaxResults={max_results:?}: expected the session to be reported, got {} {:?} \
             with {:?}",
            reply.status, reply.error_code, reply.body
        );
        assert!(
            reply.body.get("Segments").is_some(),
            "case MaxResults={max_results:?}: expected a Segments key, got {:?}",
            reply.body
        );
    }
}

/// RULE: a value breaking SEVERAL clauses states one finding per clause, so nothing a caller
/// has to fix is dropped from the message.
#[tokio::test]
async fn a_value_breaking_several_clauses_states_each_one() {
    let emulator = Emulator::new();
    emulator.seed(INSTANCE, CONTACT);
    let reply = emulator
        .list(ACCOUNT_KEY, &request(INSTANCE, Some(""), None))
        .await;
    assert_eq!(
        (reply.status, reply.error_code.as_str()),
        (400, "InvalidRequestException"),
        "case \"an empty contact id\": expected 400 InvalidRequestException, got {} {:?} with \
         {:?}",
        reply.status,
        reply.error_code,
        reply.body
    );
    let message = reply.message();
    for finding in [
        r#"string "" is too short (length: 0, required minimum: 1)"#,
        r#"ECMA 262 regex ".*\S.*" does not match input string """#,
    ] {
        assert!(
            message.contains(finding),
            "case \"an empty contact id\": expected the finding {finding:?} in the list, got \
             {message:?}"
        );
    }
    assert!(
        message.starts_with("Invalid request body: [") && message.ends_with(']'),
        "case \"an empty contact id\": expected the bracketed finding list, got {message:?}"
    );
}

// ---------------------------------------------------------------------------
// Stage 2 — IAM
// ---------------------------------------------------------------------------

/// RULE: identity-based IAM policy is evaluated before the contact is looked up, so a caller
/// the policies refuse gets `AccessDeniedException` (HTTP 403) with IAM's own sentence and
/// learns nothing about the contact — and a caller the policies allow reaches the lookup.
///
/// The rows span the kind of principal (an IAM user, an assumed-role session) and the document
/// in force on it (an explicit deny beside a service-wide allow, no document at all, an allow
/// of exactly this action).
#[tokio::test]
async fn a_refused_caller_is_told_it_is_not_authorized() {
    let emulator = Emulator::new();
    emulator.seed(INSTANCE, CONTACT);
    let resource = judged_resource(INSTANCE, CONTACT);
    let denied = allow_all_but(ACTION);

    // (case, the principal, the reason clause IAM states)
    let refused: [(&str, Principal, String); 4] = [
        (
            "a user an explicit deny covers",
            emulator.user("denied_user", Some(&denied)).await,
            "with an explicit deny in an identity-based policy".to_string(),
        ),
        (
            "an assumed-role session an explicit deny covers",
            emulator.assumed_role("denied_role", Some(&denied)).await,
            "with an explicit deny in an identity-based policy".to_string(),
        ),
        (
            "a user no document grants",
            emulator.user("ungranted_user", None).await,
            format!("because no identity-based policy allows the {ACTION} action"),
        ),
        (
            "an assumed-role session no document grants",
            emulator.assumed_role("ungranted_role", None).await,
            format!("because no identity-based policy allows the {ACTION} action"),
        ),
    ];

    for (case, principal, reason) in refused {
        let reply = emulator
            .list(
                &principal.access_key_id,
                &request(INSTANCE, Some(CONTACT), None),
            )
            .await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (403, "AccessDeniedException"),
            "case {case:?}: expected 403 AccessDeniedException, got {} {:?} with {:?}",
            reply.status,
            reply.error_code,
            reply.body
        );
        assert_eq!(
            reply.message(),
            format!(
                "User: {} is not authorized to perform: {ACTION} on resource: {resource} {reason}",
                principal.arn
            ),
            "case {case:?}: expected IAM's wording, got {:?}",
            reply.message()
        );
        assert_no_segments(case, &reply);
    }
}

/// RULE: a privileged caller issuing the identical request clears authorization and reaches
/// the lookup — the same two answers an unauthorized caller never sees.
#[tokio::test]
async fn an_allowed_caller_reaches_the_lookup() {
    let emulator = Emulator::new();
    emulator.seed(INSTANCE, CONTACT);
    let granted = emulator.user("reader", Some(&allow(ACTION))).await;
    // (case, the principal's key, the contact named, the status the lookup answers)
    let rows: [(&str, &str, &str, u16); 4] = [
        (
            "the account's own credential on the seeded pair",
            ACCOUNT_KEY,
            CONTACT,
            200,
        ),
        (
            "the account's own credential on an unknown contact",
            ACCOUNT_KEY,
            "no-such-contact",
            404,
        ),
        (
            "a user the policy allows on the seeded pair",
            &granted.access_key_id,
            CONTACT,
            200,
        ),
        (
            "a user the policy allows on an unknown contact",
            &granted.access_key_id,
            "no-such-contact",
            404,
        ),
    ];

    for (case, key, contact, expected) in rows {
        let reply = emulator
            .list(key, &request(INSTANCE, Some(contact), None))
            .await;
        assert_eq!(
            reply.status, expected,
            "case {case:?}: expected HTTP {expected}, got {} {:?} with {:?}",
            reply.status, reply.error_code, reply.body
        );
    }
}

/// RULE: authorization is decided from the request alone, so a denied caller naming a contact
/// that HAS a session and one that does not are answered identically — the refusal leaks
/// nothing about which.
#[tokio::test]
async fn the_refusal_does_not_report_whether_the_session_exists() {
    let emulator = Emulator::new();
    emulator.seed(INSTANCE, CONTACT);
    let principal = emulator
        .user("probe_user", Some(&allow_all_but(ACTION)))
        .await;

    for (case, contact) in [("a stored contact", CONTACT), ("no such contact", "absent")] {
        let reply = emulator
            .list(
                &principal.access_key_id,
                &request(INSTANCE, Some(contact), None),
            )
            .await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (403, "AccessDeniedException"),
            "case {case:?}: expected 403 AccessDeniedException, got {} {:?} with {:?}",
            reply.status,
            reply.error_code,
            reply.body
        );
        assert_eq!(
            reply.message(),
            format!(
                "User: {} is not authorized to perform: {ACTION} on resource: {} with an \
                 explicit deny in an identity-based policy",
                principal.arn,
                judged_resource(INSTANCE, contact)
            ),
            "case {case:?}: expected the refusal to name the contact the REQUEST addressed, \
             got {:?}",
            reply.message()
        );
    }
}
