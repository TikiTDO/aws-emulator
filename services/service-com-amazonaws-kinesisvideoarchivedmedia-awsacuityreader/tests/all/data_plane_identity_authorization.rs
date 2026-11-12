// SPDX-License-Identifier: CC0-1.0

//! WHO may read a stream's archived media, driven end to end over the real wire path (a
//! restJson1 request through the multi-service router into the mounted services), so what
//! is asserted is what a client holding those credentials sees.
//!
//! THE RULE UNDER TEST: every stream-addressed read of this plane is authorized against
//! the identity-based IAM policies in force on the principal its credentials name, for the
//! action `kinesisvideo:<OperationName>` on the ARN the Kinesis Video Streams control plane
//! holds the addressed stream under, BEFORE the operation body runs. A request no readable
//! statement allows (IAM's default deny), or one an explicit `Deny` covers, is answered
//! HTTP 403 `AccessDeniedException` naming the principal, the action and the stream the
//! request named, and reaches none of the operation's own refusals; a request a matching
//! `Allow` covers reaches the operation's own verdict.
//!
//! The user, the role, their inline policies and the session live in the IAM and STS stores
//! of the SAME shared engine — the stores those services' own handlers write — so the
//! documents under test are the ones `iam:PutUserPolicy` / `iam:PutRolePolicy` store, and
//! the principal a refusal blames is the one `sts:GetCallerIdentity` answers for the key
//! the request signed with.
//!
//! The tables span what the rule generalises over: the operation (all six), the document in
//! force (none, another service, another operation of this namespace, this action exactly,
//! this action in another case, the whole namespace, every action, an explicit deny beside
//! an allow), the resource a statement is scoped to (every resource, the stream addressed,
//! another stream, the same stream in another Region), how the request addresses its stream
//! (by `StreamName`, by `StreamARN`), the kind of principal (an IAM user's long-term key,
//! an assumed-role session, the account's own published credential) and the state of the
//! stream the refusal says nothing about (retained, retaining nothing, unregistered).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_kinesisvideo_kinesisvideo_20170930 as control_plane;
use service_com_amazonaws_kinesisvideoarchivedmedia_awsacuityreader as archived_media;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;
use smithy_server_runtime::MultiServiceRouter;

/// The emulator endpoint every request addresses: this emulator on loopback, never a real
/// AWS host.
const ENDPOINT: &str = "127.0.0.1:9297";

/// The region every request in this suite is signed for.
const REGION: &str = "us-east-1";

/// The action namespace this endpoint's operations are published under: its `sigv4Name` and
/// `arnNamespace` (`smithy_query(query="service_info")`), which the Kinesis Video Streams
/// control plane shares.
const ACTION_PREFIX: &str = "kinesisvideo";

/// The access key id of the IAM user the long-term rows sign with.
const USER_KEY_ID: &str = "AKIAEXAMPLEKVAMREADER";

/// The access key id of the assumed-role session the role rows sign with.
const SESSION_KEY_ID: &str = "ASIAEXAMPLEKVAMSESSON";

/// An access key id neither store holds a record for: the account's own published
/// credential, which holds no IAM policy.
const AMBIENT_KEY_ID: &str = "AKIDEXAMPLE";

/// The IAM user, the role the session is assumed from, the session's own name, and the name
/// of the inline policy each of them carries.
const USER: &str = "archived-media-reader";
const ROLE: &str = "archived-media-reader-role";
const SESSION: &str = "read-session";
const POLICY_NAME: &str = "inline";

/// How long the seeded session lasts: an hour, STS's own default.
const SESSION_SECONDS: i32 = 3_600;

/// The error code and status an unauthorized call is answered with.
const ACCESS_DENIED: (u16, &str) = (403, "AccessDeniedException");

/// The stream every row reads unless it says otherwise, and a second one for the rows about
/// a statement's scope.
const STREAM: &str = "sensor-feed";
const OTHER_STREAM: &str = "door-camera";

/// A stream the control plane holds with `DataRetentionInHours` of 0, which the read plane's
/// own verdict refuses — reached only once authorization allows the call.
const NO_RETENTION_STREAM: &str = "unretained-feed";

/// The base instant the selectors in this suite reach from.
const T0_SECS: i64 = 1_761_000_000;

/// The ARN the account holds the IAM user under, which a refusal names as that caller.
fn user_arn() -> String {
    format!(
        "arn:aws:iam::{}:user/{USER}",
        engine::emulator_account_id()
    )
}

/// The ARN the account holds the role under: the identity whose policies an assumed-role
/// session's request is judged on.
fn role_arn() -> String {
    format!(
        "arn:aws:iam::{}:role/{ROLE}",
        engine::emulator_account_id()
    )
}

/// The principal ARN the seeded session speaks for — what a refusal must name.
fn session_arn() -> String {
    format!(
        "arn:aws:sts::{}:assumed-role/{ROLE}/{SESSION}",
        engine::emulator_account_id()
    )
}

/// One router over one engine, carrying the archived-media reader AND the Kinesis Video
/// Streams control plane, so a stream created through `CreateStream` is the very record the
/// reads resolve and the ARN an IAM statement is scoped to.
struct Harness {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(control_plane::EmulatedService::new(Arc::clone(&engine)));
        router.register_service(archived_media::EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// POST one restJson1 call signed with `key_id`, and answer its status, its error code
    /// and its parsed body.
    async fn post_as(&self, key_id: &str, path: &str, body: &Value) -> Reply {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{ENDPOINT}{path}"))
            .header("host", ENDPOINT)
            .header("content-type", "application/json")
            // The SigV4 credential scope routes the request, names its region and names
            // the identity it speaks for; the signature itself is not checked here.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={key_id}/20260101/{REGION}/kinesisvideo/\
                     aws4_request, SignedHeaders=host, Signature=deadbeef"
                ),
            )
            .body(bytes::Bytes::from(body.to_string()))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let body = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        let code = body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or(header_code.as_str())
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string();
        Reply { status, code, body }
    }

    /// Create `name` through the control plane with `retention` hours, wait for it to
    /// be readable, and answer its `StreamARN` — the ARN an IAM statement names that
    /// stream by.
    async fn create_stream(&self, name: &str, retention: i32) -> String {
        let reply = self
            .post_as(
                AMBIENT_KEY_ID,
                "/createStream",
                &json!({ "StreamName": name, "DataRetentionInHours": retention }),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case \"create {name}\": expected HTTP 200, got {} with body {}",
            reply.status, reply.body
        );
        let arn = reply.body["StreamARN"]
            .as_str()
            .unwrap_or_else(|| panic!("case \"create {name}\": no StreamARN in {}", reply.body))
            .to_string();
        self.await_active(name).await;
        arn
    }

    /// Wait until the control plane reports `name` as `ACTIVE`, the one status an
    /// archived-media read serves, as a client of the real service does before reading
    /// a stream it just created ("CreateStream is an asynchronous operation").
    ///
    /// Waits for the CONDITION, not for a span: it looks again until the status holds,
    /// under a deadline far longer than the transition needs.
    async fn await_active(&self, name: &str) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let mut last = String::new();
        while std::time::Instant::now() < deadline {
            let reply = self
                .post_as(AMBIENT_KEY_ID, "/describeStream", &json!({ "StreamName": name }))
                .await;
            last = reply.body["StreamInfo"]["Status"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            if last == "ACTIVE" {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("case \"create {name}\": expected Status ACTIVE before the deadline, got {last:?}");
    }
}

/// What one call answered.
#[derive(Debug)]
struct Reply {
    status: u16,
    code: String,
    body: Value,
}

impl Reply {
    /// The `Message`/`message` the answer states, whichever member carries it: the
    /// authorization refusal spells it in upper camel case, this service's modelled errors
    /// in lower.
    fn message(&self) -> String {
        self.body["Message"]
            .as_str()
            .or_else(|| self.body["message"].as_str())
            .unwrap_or_default()
            .to_string()
    }
}

/// Record the IAM user [`USER_KEY_ID`] belongs to, as `iam:CreateUser` and
/// `iam:CreateAccessKey` record one: the user, and the key record that maps the access key
/// id to it.
fn given_user(engine: &Arc<engine::InMemoryEngine>) {
    let minted = aws_smithy_types::DateTime::from_secs(0);
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    store.users.insert(
        iam::name_key(USER),
        service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::User {
            path: "/".to_string(),
            user_name: USER.to_string(),
            user_id: "AIDAEXAMPLEKVAMREADER1".to_string(),
            arn: user_arn(),
            create_date: minted,
            password_last_used: None,
            permissions_boundary: None,
            tags: None,
        },
    );
    store.access_keys.insert(
        USER_KEY_ID.to_string(),
        iam::AccessKeyRecord {
            user_key: iam::name_key(USER),
            status: iam::ACCESS_KEY_ACTIVE_STATUS.to_string(),
            create_date: minted,
        },
    );
}

/// Record the role and the assumed-role session [`SESSION_KEY_ID`] speaks for, as
/// `iam:CreateRole` and `sts:AssumeRole` record them.
fn given_session(engine: &Arc<engine::InMemoryEngine>) {
    let role_arn = role_arn();
    let iam_store = iam::service_state(&**engine).expect("the IAM store");
    iam_store.lock().roles.insert(
        iam::name_key(ROLE),
        service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role {
            path: "/".to_string(),
            role_name: ROLE.to_string(),
            role_id: "AROAEXAMPLEKVAMREADER1".to_string(),
            arn: role_arn.clone(),
            create_date: aws_smithy_types::DateTime::from_secs(0),
            assume_role_policy_document: None,
            description: None,
            max_session_duration: None,
            permissions_boundary: None,
            tags: None,
            role_last_used: None,
            source_role_template: None,
        },
    );
    let vend = sts::NewSession {
        role_arn: Some(role_arn.clone()),
        ..sts::NewSession::new(
            sts::Identity {
                user_id: sts::role_id(&role_arn),
                account: engine::emulator_account_id().to_string(),
                arn: session_arn(),
            },
            sts::SessionKind::AssumedRole,
            SESSION_SECONDS,
        )
    };
    let store = sts::service_state(&**engine).expect("the STS store");
    store
        .lock()
        .insert(vend.into_session(SESSION_KEY_ID, SystemTime::now()))
        .expect("the session is recorded");
}

/// One row of the principal table: the case, the access key id it signs with, and — for a
/// principal the IAM store holds policies for — that principal's ARN together with the
/// identity a grant has to be attached to for the call to be served. `None` is the
/// account's own published credential, which holds no IAM policy.
type PrincipalRow = (&'static str, &'static str, Option<(String, Target)>);

/// Which identity an inline policy is attached to.
#[derive(Debug, Clone, Copy)]
enum Target {
    User,
    Role,
}

/// Put `document` in force as that identity's inline policy, replacing whatever it carried
/// — what `iam:PutUserPolicy` / `iam:PutRolePolicy` do. `None` takes every policy off it,
/// as the matching delete does.
fn given_policy(engine: &Arc<engine::InMemoryEngine>, target: Target, document: Option<&Value>) {
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    let (key, attached) = match target {
        Target::User => (iam::name_key(USER), &mut store.user_inline_policies),
        Target::Role => (iam::name_key(ROLE), &mut store.role_inline_policies),
    };
    match document {
        None => {
            attached.remove(&key);
        }
        Some(document) => {
            attached.insert(
                key,
                BTreeMap::from([(
                    iam::name_key(POLICY_NAME),
                    iam::InlinePolicy {
                        policy_name: POLICY_NAME.to_string(),
                        policy_document: document.to_string(),
                    },
                )]),
            );
        }
    }
}

/// A policy allowing `actions` on every resource.
fn allow(actions: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": actions, "Resource": "*" }]
    })
}

/// A policy allowing every action of this namespace and denying `action` on `resource`
/// alone — the shape of a scoped denial.
fn allow_all_but(action: &str, resource: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            { "Effect": "Allow", "Action": format!("{ACTION_PREFIX}:*"), "Resource": "*" },
            { "Effect": "Deny", "Action": action, "Resource": resource },
        ]
    })
}

/// Every operation of this read plane, with the request body it carries once the member
/// addressing the stream is added: the operation name (which is also the action's), its
/// path, and that body.
fn operations() -> Vec<(&'static str, &'static str, Value)> {
    vec![
        (
            "ListFragments",
            "/listFragments",
            json!({
                "FragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": {
                        "StartTimestamp": T0_SECS,
                        "EndTimestamp": T0_SECS + 1000,
                    },
                },
            }),
        ),
        (
            "GetMediaForFragmentList",
            "/getMediaForFragmentList",
            json!({ "Fragments": ["9134385233318143200"] }),
        ),
        (
            "GetClip",
            "/getClip",
            json!({
                "ClipFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": {
                        "StartTimestamp": T0_SECS,
                        "EndTimestamp": T0_SECS + 1000,
                    },
                },
            }),
        ),
        (
            "GetHLSStreamingSessionURL",
            "/getHLSStreamingSessionURL",
            json!({
                "PlaybackMode": "ON_DEMAND",
                "HLSFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": {
                        "StartTimestamp": T0_SECS,
                        "EndTimestamp": T0_SECS + 1000,
                    },
                },
            }),
        ),
        (
            "GetDASHStreamingSessionURL",
            "/getDASHStreamingSessionURL",
            json!({
                "PlaybackMode": "ON_DEMAND",
                "DASHFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": {
                        "StartTimestamp": T0_SECS,
                        "EndTimestamp": T0_SECS + 1000,
                    },
                },
            }),
        ),
        (
            "GetImages",
            "/getImages",
            json!({
                "ImageSelectorType": "PRODUCER_TIMESTAMP",
                "StartTimestamp": T0_SECS,
                "EndTimestamp": T0_SECS + 10,
                "Format": "PNG",
            }),
        ),
    ]
}

/// `base` with `member` bound to `value`.
fn addressing(base: &Value, member: &str, value: &str) -> Value {
    let mut object = base.as_object().cloned().unwrap_or_default();
    object.insert(member.to_string(), Value::String(value.to_string()));
    Value::Object(object)
}

/// The sentence an unauthorized call is refused with: who was refused, which action, and
/// the stream the request named. Spelled out here rather than built from the helper the
/// implementation calls, so a change to the wording fails this test.
fn denial(principal: &str, operation: &str, resource: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {ACTION_PREFIX}:{operation} on \
         resource: {resource}"
    )
}

/// Assert that `reply` is the authorization refusal naming `principal`, `operation` and
/// `resource`.
fn assert_refused(case: &str, reply: &Reply, principal: &str, operation: &str, resource: &str) {
    assert_eq!(
        (reply.status, reply.code.as_str()),
        ACCESS_DENIED,
        "case {case:?}: expected {ACCESS_DENIED:?}, got {} {:?} with body {}",
        reply.status,
        reply.code,
        reply.body
    );
    let stated = reply.message();
    let expected = denial(principal, operation, resource);
    assert_eq!(
        stated, expected,
        "case {case:?}: expected {expected:?}, got {stated:?}"
    );
}

/// Assert that `reply` is NOT an authorization refusal: the call reached the operation's
/// own verdict, whatever that verdict is.
fn assert_allowed(case: &str, reply: &Reply) {
    assert_ne!(
        (reply.status, reply.code.as_str()),
        ACCESS_DENIED,
        "case {case:?}: an allowed call must reach the operation's own verdict, got body {}",
        reply.body
    );
}

/// THE RULE, over every operation of the plane: each is authorized against
/// `kinesisvideo:<its own name>` on the addressed stream's ARN, so an explicit deny of that
/// one action refuses that one operation with AWS's sentence, and a grant of the namespace
/// lets every one of them reach its own verdict.
#[tokio::test]
async fn every_operation_is_authorized_against_its_own_action() {
    for (operation, path, body) in operations() {
        let action = format!("{ACTION_PREFIX}:{operation}");

        // Denied: everything of this namespace allowed, except this action on this stream.
        let harness = Harness::new();
        given_user(&harness.engine);
        let arn = harness.create_stream(STREAM, 24).await;
        given_policy(
            &harness.engine,
            Target::User,
            Some(&allow_all_but(&action, &arn)),
        );
        let request = addressing(&body, "StreamName", STREAM);
        let reply = harness.post_as(USER_KEY_ID, path, &request).await;
        assert_refused(
            &format!("{operation} / explicitly denied on this stream"),
            &reply,
            &user_arn(),
            operation,
            STREAM,
        );

        // Allowed: the same call, with the whole namespace granted and nothing denied.
        let harness = Harness::new();
        given_user(&harness.engine);
        harness.create_stream(STREAM, 24).await;
        given_policy(
            &harness.engine,
            Target::User,
            Some(&allow(json!(format!("{ACTION_PREFIX}:*")))),
        );
        let reply = harness.post_as(USER_KEY_ID, path, &request).await;
        assert_allowed(&format!("{operation} / the namespace granted"), &reply);
    }
}

/// RULE: the verdict is read off the documents in force on the signing principal, judged
/// against `kinesisvideo:<OperationName>` — so a grant that does not name that action (none
/// at all, another service, another operation of the same namespace) refuses the call, and
/// an explicit deny refuses it whatever an allow beside it says.
#[tokio::test]
async fn a_grant_must_name_the_action_the_call_performs() {
    // (case, the document in force, whether the call is refused)
    let rows: Vec<(&str, Option<Value>, bool)> = vec![
        ("no policy at all", None, true),
        (
            "every action of another service",
            Some(allow(json!("kinesis:*"))),
            true,
        ),
        (
            "a prefix that only starts like this one",
            Some(allow(json!("kinesisvideoarchivedmedia:*"))),
            true,
        ),
        (
            "another operation of this namespace",
            Some(allow(json!(format!("{ACTION_PREFIX}:ListFragments")))),
            true,
        ),
        (
            "an explicit deny of this action beside an allow of it",
            Some(json!({
                "Statement": [
                    { "Effect": "Allow", "Action": format!("{ACTION_PREFIX}:GetImages"),
                      "Resource": "*" },
                    { "Effect": "Deny", "Action": format!("{ACTION_PREFIX}:GetImages"),
                      "Resource": "*" },
                ],
            })),
            true,
        ),
        (
            "exactly this action",
            Some(allow(json!(format!("{ACTION_PREFIX}:GetImages")))),
            false,
        ),
        (
            // IAM compares action names case-insensitively, so the spelling of the allow
            // is not what decides the request.
            "this action spelled in lower case",
            Some(allow(json!(format!("{ACTION_PREFIX}:getimages")))),
            false,
        ),
        (
            "this action listed beside another service's",
            Some(allow(json!(["kinesis:GetRecords", "kinesisvideo:GetImages"]))),
            false,
        ),
        (
            "every action of this namespace",
            Some(allow(json!(format!("{ACTION_PREFIX}:*")))),
            false,
        ),
        ("every action", Some(allow(json!("*"))), false),
    ];

    let (_, path, body) = operations()
        .into_iter()
        .find(|(operation, _, _)| *operation == "GetImages")
        .expect("GetImages is one of this plane's operations");
    let request = addressing(&body, "StreamName", STREAM);

    for (case, document, refused) in rows {
        // A fresh engine per row: no row can pass on another's write.
        let harness = Harness::new();
        given_user(&harness.engine);
        harness.create_stream(STREAM, 24).await;
        given_policy(&harness.engine, Target::User, document.as_ref());

        let reply = harness.post_as(USER_KEY_ID, path, &request).await;
        if refused {
            assert_refused(case, &reply, &user_arn(), "GetImages", STREAM);
        } else {
            assert_allowed(case, &reply);
        }
    }
}

/// RULE: the decision is taken against the ARN the control plane holds the addressed stream
/// under, so a statement scoped to one stream's ARN covers that stream and no other — and
/// it covers it however the request addresses it, by `StreamName` or by `StreamARN`.
#[tokio::test]
async fn a_statement_is_scoped_to_the_stream_the_request_addresses() {
    let harness = Harness::new();
    given_user(&harness.engine);
    let arn = harness.create_stream(STREAM, 24).await;
    let other_arn = harness.create_stream(OTHER_STREAM, 24).await;
    given_policy(
        &harness.engine,
        Target::User,
        Some(&allow_all_but(&format!("{ACTION_PREFIX}:GetImages"), &arn)),
    );

    let (_, path, body) = operations()
        .into_iter()
        .find(|(operation, _, _)| *operation == "GetImages")
        .expect("GetImages is one of this plane's operations");

    // (case, the member addressing the stream, its value, whether the deny covers the call)
    let rows: Vec<(&str, &str, String, bool)> = vec![
        (
            "the denied stream, by name",
            "StreamName",
            STREAM.to_string(),
            true,
        ),
        (
            "the denied stream, by ARN",
            "StreamARN",
            arn.clone(),
            true,
        ),
        (
            "another stream of the same account and Region, by name",
            "StreamName",
            OTHER_STREAM.to_string(),
            false,
        ),
        (
            "another stream of the same account and Region, by ARN",
            "StreamARN",
            other_arn.clone(),
            false,
        ),
    ];

    for (case, member, value, covered) in rows {
        let reply = harness
            .post_as(USER_KEY_ID, path, &addressing(&body, member, &value))
            .await;
        if covered {
            assert_refused(case, &reply, &user_arn(), "GetImages", &value);
        } else {
            assert_allowed(case, &reply);
        }
    }

    // The deny is scoped to the ACTION too: the same identity's ListFragments on the very
    // stream whose GetImages is denied is allowed, and reaches that operation's own answer.
    let (_, list_path, list_body) = operations()
        .into_iter()
        .find(|(operation, _, _)| *operation == "ListFragments")
        .expect("ListFragments is one of this plane's operations");
    let reply = harness
        .post_as(
            USER_KEY_ID,
            list_path,
            &addressing(&list_body, "StreamName", STREAM),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case \"an action the deny does not name\": expected HTTP 200, got {} with body {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.body["Fragments"],
        json!([]),
        "case \"an action the deny does not name\": a registered stream nothing was ingested \
         into lists no fragment, got {}",
        reply.body
    );
}

/// RULE: the refusal is decided before the stream is resolved, so it says nothing about
/// that stream — the same identical answer whether the stream retains data, retains
/// nothing, or is not registered at all. Once the action IS granted, those three calls
/// reach three DIFFERENT verdicts of the operation's own, which is what shows the gate ran
/// ahead of them.
#[tokio::test]
async fn a_refusal_says_nothing_about_the_stream_it_withholds() {
    let harness = Harness::new();
    given_user(&harness.engine);
    harness.create_stream(STREAM, 24).await;
    harness.create_stream(NO_RETENTION_STREAM, 0).await;
    let absent = "nothing-registered-here";

    let (_, path, body) = operations()
        .into_iter()
        .find(|(operation, _, _)| *operation == "GetImages")
        .expect("GetImages is one of this plane's operations");

    // (case, the stream the request names, the code its own verdict answers once granted)
    let rows: [(&str, &str, &str); 3] = [
        ("a stream that retains data", STREAM, "ResourceNotFoundException"),
        (
            "a stream that retains nothing",
            NO_RETENTION_STREAM,
            "NoDataRetentionException",
        ),
        (
            "a stream nothing registered",
            absent,
            "ResourceNotFoundException",
        ),
    ];

    // Nothing granted: one refusal, differing only in the stream it names.
    given_policy(&harness.engine, Target::User, None);
    let mut refusals: Vec<(&str, String)> = Vec::new();
    for (case, stream, _) in rows {
        let reply = harness
            .post_as(USER_KEY_ID, path, &addressing(&body, "StreamName", stream))
            .await;
        assert_refused(case, &reply, &user_arn(), "GetImages", stream);
        refusals.push((case, reply.body.to_string().replace(stream, "<stream>")));
    }
    let (first_case, first) = &refusals[0];
    for (case, refusal) in &refusals[1..] {
        assert_eq!(
            refusal, first,
            "case {case:?}: the refusal must not differ from case {first_case:?} by anything \
             but the stream it names, got {refusal}"
        );
    }

    // Granted: the identical calls now reach the operation's own verdict for each stream.
    given_policy(
        &harness.engine,
        Target::User,
        Some(&allow(json!(format!("{ACTION_PREFIX}:*")))),
    );
    for (case, stream, expected) in rows {
        let reply = harness
            .post_as(USER_KEY_ID, path, &addressing(&body, "StreamName", stream))
            .await;
        assert_eq!(
            reply.code, expected,
            "case {case:?}: expected the operation's own {expected}, got {} {:?} with body {}",
            reply.status, reply.code, reply.body
        );
    }
}

/// RULE: the rule holds for every kind of principal a request can present — an IAM user's
/// long-term key is judged on the USER's policies, an assumed-role session on the ROLE's,
/// and the account's own published credential (a key neither store holds a record for) is
/// not a principal the IAM store holds policies for and is served.
#[tokio::test]
async fn each_kind_of_principal_is_judged_on_its_own_policies() {
    let (_, path, body) = operations()
        .into_iter()
        .find(|(operation, _, _)| *operation == "GetImages")
        .expect("GetImages is one of this plane's operations");
    let request = addressing(&body, "StreamName", STREAM);
    let grant = allow(json!(format!("{ACTION_PREFIX}:*")));

    // (case, the key the call signs with, the principal a refusal names, which identity a
    // grant must be attached to for the call to be served)
    let rows: [PrincipalRow; 3] = [
        (
            "an IAM user's long-term key",
            USER_KEY_ID,
            Some((user_arn(), Target::User)),
        ),
        (
            "an assumed-role session",
            SESSION_KEY_ID,
            Some((session_arn(), Target::Role)),
        ),
        (
            "the account's own published credential",
            AMBIENT_KEY_ID,
            None,
        ),
    ];

    for (case, key_id, judged) in rows {
        // Nothing granted to either identity, so only the kind of principal differs.
        let harness = Harness::new();
        given_user(&harness.engine);
        given_session(&harness.engine);
        harness.create_stream(STREAM, 24).await;

        let reply = harness.post_as(key_id, path, &request).await;
        match &judged {
            Some((principal, _)) => {
                assert_refused(case, &reply, principal, "GetImages", STREAM);
            }
            None => assert_allowed(case, &reply),
        }

        // The grant in force is the one attached to the principal that signed: granting the
        // OTHER identity leaves this call refused, granting its own serves it.
        if let Some((principal, target)) = judged {
            let other = match target {
                Target::User => Target::Role,
                Target::Role => Target::User,
            };
            given_policy(&harness.engine, other, Some(&grant));
            let reply = harness.post_as(key_id, path, &request).await;
            assert_refused(
                &format!("{case} / the other identity granted"),
                &reply,
                &principal,
                "GetImages",
                STREAM,
            );

            given_policy(&harness.engine, target, Some(&grant));
            let reply = harness.post_as(key_id, path, &request).await;
            assert_allowed(&format!("{case} / its own identity granted"), &reply);
        }
    }
}

/// RULE: the decision is re-read per request, so granting the action serves the very call
/// that was refused and revoking it refuses the very call that was served — and a refused
/// call reaches none of the operation's own work.
#[tokio::test]
async fn granting_the_action_serves_the_very_call_that_was_refused() {
    let harness = Harness::new();
    given_user(&harness.engine);
    harness.create_stream(STREAM, 24).await;
    let (_, path, body) = operations()
        .into_iter()
        .find(|(operation, _, _)| *operation == "ListFragments")
        .expect("ListFragments is one of this plane's operations");
    let request = addressing(&body, "StreamName", STREAM);

    // Everything of another service, which grants no action of this one.
    given_policy(&harness.engine, Target::User, Some(&allow(json!("kinesis:*"))));
    let refused = harness.post_as(USER_KEY_ID, path, &request).await;
    assert_refused(
        "another service's namespace",
        &refused,
        &user_arn(),
        "ListFragments",
        STREAM,
    );

    given_policy(
        &harness.engine,
        Target::User,
        Some(&allow(json!(format!("{ACTION_PREFIX}:ListFragments")))),
    );
    let served = harness.post_as(USER_KEY_ID, path, &request).await;
    assert_eq!(
        served.status, 200,
        "case \"the action granted\": the identical call must be served, got {} with body {}",
        served.status, served.body
    );
    assert_eq!(
        served.body["Fragments"],
        json!([]),
        "case \"the action granted\": a registered stream nothing was ingested into lists no \
         fragment, got {}",
        served.body
    );

    // Revoked again: the decision is read per request, not cached from the served one.
    given_policy(&harness.engine, Target::User, None);
    let refused_again = harness.post_as(USER_KEY_ID, path, &request).await;
    assert_refused(
        "the grant revoked",
        &refused_again,
        &user_arn(),
        "ListFragments",
        STREAM,
    );
}
