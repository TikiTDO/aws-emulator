// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets a call to this signaling data plane through, driven end to end over
//! the wire with the services that hold the identity and its policies.
//!
//! The rule these cases assert, one level above the recorded case (one principal without
//! the permission, one call refused):
//!
//! > Both operations of this service are authorized against the IDENTITY-BASED policies in
//! > force on the principal the request's credential names, on the action
//! > `kinesisvideo:<OperationName>` and the channel ARN the request addresses: a call no
//! > statement allows (IAM's default deny) and one an explicit `Deny` covers are both
//! > refused `AccessDeniedException` (HTTP 403) with nothing read, minted or delivered,
//! > while a call a statement allows is served. The account's own published credential,
//! > which holds no IAM policies, is the position AWS account root credentials are in and
//! > is served.
//!
//! The rows span what the rule generalises over: the statements a policy can carry (an
//! allow of the exact action, a wildcard allow, an allow of the OTHER operation, an
//! explicit deny, a resource that is another channel, no policy at all), and both
//! operations.

use std::sync::Arc;

use bytes::Bytes;
use engine::InMemoryEngine;
use serde_json::{json, Value};
use service_com_amazonaws_kinesisvideosignaling_awsacuitysignalingservice as signaling;
use smithy_server_runtime::MultiServiceRouter;

/// The region every row's resources are created and addressed in.
const REGION: &str = "us-east-1";

/// The emulator's own published long-term access key: the account credential, which holds
/// no IAM policies.
const ACCOUNT_KEY: &str = "AKIDEXAMPLE";

/// The channel every call addresses, and another one a policy can be scoped to instead.
const CHANNEL: &str = "arn:aws:kinesisvideo:us-east-1:000000000000:channel/authorized/1700000000";
const OTHER_CHANNEL: &str =
    "arn:aws:kinesisvideo:us-east-1:000000000000:channel/elsewhere/1700000001";

const GET_ICE_SERVER_CONFIG: &str = "/v1/get-ice-server-config";
const SEND_ALEXA_OFFER_TO_MASTER: &str = "/v1/send-alexa-offer-to-master";

/// The router every case drives: this service, plus the two that own the identity a request
/// speaks for (STS) and the policies in force on it (IAM), all on ONE engine.
struct Harness {
    engine: Arc<InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(signaling::EmulatedService::new(Arc::clone(&engine)));
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
        Self { engine, router }
    }

    /// One signaling call, signed with `access_key_id`.
    async fn signaling(&self, access_key_id: &str, path: &str, body: Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://kinesisvideo.{REGION}.amazonaws.com{path}"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/kinesisvideo/\
                     aws4_request, SignedHeaders=host, Signature=0"
                ),
            )
            .body(Bytes::from(
                serde_json::to_vec(&body).expect("the test body serializes"),
            ))
            .expect("the test request builds");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let mut parsed: Value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        if let (Some(error_type), Some(object)) = (error_type, parsed.as_object_mut()) {
            object
                .entry("__type".to_string())
                .or_insert(Value::String(error_type));
        }
        (status, parsed)
    }

    /// One awsQuery call to IAM or STS: `signing_name` picks the service, `version` its API
    /// version.
    async fn query(
        &self,
        signing_name: &str,
        version: &str,
        action: &str,
        params: &[(&str, String)],
    ) -> (u16, String) {
        let mut body = format!("Action={action}&Version={version}");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encoded(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{signing_name}.amazonaws.com/"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/{signing_name}/\
                     aws4_request, SignedHeaders=host, Signature=0"
                ),
            )
            .body(Bytes::from(body))
            .expect("the test request builds");
        let response = self.router.dispatch(request).await;
        (
            response.status().as_u16(),
            String::from_utf8_lossy(&response.into_body()).to_string(),
        )
    }

    /// Create an IAM role carrying `policy` as its inline policy (when one is given),
    /// assume it through STS, and answer the access key id of the session — the credential
    /// a caller then signs its signaling requests with.
    async fn assumed_role_key(&self, role_name: &str, policy: Option<&str>) -> String {
        let account = engine::EMULATOR_ACCOUNT_ID;
        let trust = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": { "AWS": format!("arn:aws:iam::{account}:root") },
                "Action": "sts:AssumeRole",
            }],
        })
        .to_string();
        let (status, body) = self
            .query(
                "iam",
                "2010-05-08",
                "CreateRole",
                &[
                    ("RoleName", role_name.to_string()),
                    ("AssumeRolePolicyDocument", trust),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "setup: creating the role {role_name:?} must succeed, got {status} {body}"
        );
        if let Some(policy) = policy {
            let (status, body) = self
                .query(
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
            assert_eq!(
                status, 200,
                "setup: putting the inline policy of {role_name:?} must succeed, got \
                 {status} {body}"
            );
        }
        let (status, body) = self
            .query(
                "sts",
                "2011-06-15",
                "AssumeRole",
                &[
                    (
                        "RoleArn",
                        format!("arn:aws:iam::{account}:role/{role_name}"),
                    ),
                    ("RoleSessionName", "probe".to_string()),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "setup: assuming {role_name:?} must succeed, got {status} {body}"
        );
        xml_element(&body, "AccessKeyId")
            .unwrap_or_else(|| panic!("setup: AssumeRole must answer an AccessKeyId, got {body}"))
    }
}

/// One form field, percent-encoded the way an awsQuery body carries it.
fn form_encoded(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char)
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// The text of the first `<name>` element of an awsQuery XML response.
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// An identity-based policy with one statement.
fn policy(effect: &str, actions: &[&str], resources: &[&str]) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": effect,
            "Action": actions,
            "Resource": resources,
        }],
    })
    .to_string()
}

/// The error shape a failure answered with.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .map(|code| code.rsplit(['#', '/']).next().unwrap_or(code).to_string())
        .unwrap_or_else(|| format!("no __type in {body}"))
}

/// Attach a MASTER peer to `arn` that answers every offer delivered to it with `answer`.
///
/// The device at the other end of the channel's WebSocket data plane, driven through the
/// SAME session a real master's socket task drives, so a permitted `SendAlexaOfferToMaster`
/// is answered at once instead of waiting out the channel's window.
fn attach_answering_master(
    engine: &InMemoryEngine,
    arn: &str,
    answer: &str,
) -> tokio::task::JoinHandle<()> {
    let (peer, mut queued) = signaling::state::PeerConnection::new();
    let state = engine.state::<signaling::state::ComAmazonawsKinesisvideosignalingAwsacuitysignalingserviceState>();
    {
        let mut store = state.lock();
        let channel = store
            .resolve_channel_mut(arn)
            .unwrap_or_else(|error| panic!("setup: {arn:?}: {error:?}"));
        assert!(
            channel.session.attach_master(peer).is_some(),
            "setup: {arn:?}: the channel must take a master"
        );
    }
    let answer = answer.to_string();
    let arn = arn.to_string();
    tokio::spawn(async move {
        while let Some(frame) = queued.recv().await {
            let parsed: Value = serde_json::from_str(&frame)
                .unwrap_or_else(|error| panic!("a delivered frame must be JSON ({error}): {frame}"));
            let sender = parsed
                .get("senderClientId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let mut store = state.lock();
            if let Ok(channel) = store.resolve_channel_mut(&arn) {
                channel.session.deliver_answer(&sender, &answer);
            }
        }
    })
}

/// The code and status AWS refuses a call IAM does not permit with — the pair recorded
/// behaviour shows, which this service's model declares no shape for.
const DENIED_CODE: &str = "AccessDeniedException";
const DENIED_STATUS: u16 = 403;

/// One row: the policy a principal holds, and the status each operation owes it.
struct Row {
    case: &'static str,
    role: &'static str,
    /// The inline policy the role carries; `None` for a principal the account grants
    /// nothing.
    policy: Option<String>,
    /// The status `GetIceServerConfig` owes this caller.
    ice_status: u16,
    /// The status `SendAlexaOfferToMaster` owes it.
    offer_status: u16,
    /// Whether a document of this principal's EXPLICITLY denies the call, which is the
    /// rule a refusal names — as against IAM's default deny, where no document refused it
    /// and none allowed it.
    explicitly_denied: bool,
}

/// Both operations are served only to a caller IAM permits.
#[tokio::test]
async fn a_signaling_call_is_served_only_to_a_caller_iam_permits() {
    let harness = Harness::new();
    // Both channels exist: what a row varies is the POLICY, never whether the resource is
    // there, so a refusal is never a disguised not-found.
    for arn in [CHANNEL, OTHER_CHANNEL] {
        assert_eq!(
            signaling::state::register_channel_of(&*harness.engine, arn, 5),
            signaling::state::ChannelRegistration::Registered,
            "setup: the registry must accept {arn:?}"
        );
    }
    // A master listens on the channel every row addresses and answers whatever offer
    // reaches it, so a row IAM permits is answered by that master rather than waiting out
    // the channel's redelivery window.
    let _master = attach_answering_master(&harness.engine, CHANNEL, "dj0wDQphPXJlY3Zvbmx5DQo=");

    let rows = vec![
        Row {
            case: "a policy allowing both actions on the channel",
            role: "both-actions",
            policy: Some(policy(
                "Allow",
                &[
                    "kinesisvideo:GetIceServerConfig",
                    "kinesisvideo:SendAlexaOfferToMaster",
                ],
                &[CHANNEL],
            )),
            ice_status: 200,
            offer_status: 200,
            explicitly_denied: false,
        },
        Row {
            case: "a wildcard allow of every action on every resource",
            role: "wildcard",
            policy: Some(policy("Allow", &["kinesisvideo:*"], &["*"])),
            ice_status: 200,
            offer_status: 200,
            explicitly_denied: false,
        },
        Row {
            case: "a policy allowing only the ICE action",
            role: "ice-only",
            policy: Some(policy(
                "Allow",
                &["kinesisvideo:GetIceServerConfig"],
                &["*"],
            )),
            ice_status: 200,
            offer_status: DENIED_STATUS,
            explicitly_denied: false,
        },
        Row {
            case: "an allow scoped to another channel",
            role: "other-channel",
            policy: Some(policy("Allow", &["kinesisvideo:*"], &[OTHER_CHANNEL])),
            ice_status: DENIED_STATUS,
            offer_status: DENIED_STATUS,
            explicitly_denied: false,
        },
        Row {
            case: "an explicit deny beside no allow",
            role: "explicit-deny",
            policy: Some(policy("Deny", &["kinesisvideo:*"], &["*"])),
            ice_status: DENIED_STATUS,
            offer_status: DENIED_STATUS,
            explicitly_denied: true,
        },
        Row {
            case: "an explicit deny of this action beside an allow of every action",
            role: "deny-wins",
            policy: Some(
                json!({
                    "Version": "2012-10-17",
                    "Statement": [
                        { "Effect": "Allow", "Action": "kinesisvideo:*", "Resource": "*" },
                        {
                            "Effect": "Deny",
                            "Action": "kinesisvideo:GetIceServerConfig",
                            "Resource": CHANNEL,
                        },
                    ],
                })
                .to_string(),
            ),
            ice_status: DENIED_STATUS,
            offer_status: 200,
            explicitly_denied: true,
        },
        Row {
            case: "no policy at all, which is IAM's default deny",
            role: "no-policy",
            policy: None,
            ice_status: DENIED_STATUS,
            offer_status: DENIED_STATUS,
            explicitly_denied: false,
        },
    ];

    for row in &rows {
        let case = row.case;
        let key = harness
            .assumed_role_key(row.role, row.policy.as_deref())
            .await;

        let (status, answered) = harness
            .signaling(
                &key,
                GET_ICE_SERVER_CONFIG,
                json!({ "ChannelARN": CHANNEL, "ClientId": "viewer-1" }),
            )
            .await;
        assert_eq!(
            status, row.ice_status,
            "case {case:?}: GetIceServerConfig expected HTTP {}, got {status} with {answered}",
            row.ice_status
        );
        if row.ice_status == DENIED_STATUS {
            assert_eq!(
                error_code(&answered),
                DENIED_CODE,
                "case {case:?}: expected {DENIED_CODE}, got {answered}"
            );
            let message = answered
                .get("Message")
                .or_else(|| answered.get("message"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            // The sentence AWS words this refusal in: the principal, the action, the
            // resource, and which of IAM's rules refused it — an explicit deny is NAMED,
            // and IAM's default deny states the action no policy allows.
            let expected_tail = match row.explicitly_denied {
                true => "with an explicit deny in an identity-based policy".to_string(),
                false => "because no identity-based policy allows the \
                          kinesisvideo:GetIceServerConfig action"
                    .to_string(),
            };
            assert!(
                message.starts_with("User: ")
                    && message.contains(
                        " is not authorized to perform: kinesisvideo:GetIceServerConfig on \
                         resource: "
                    )
                    && message.contains(CHANNEL)
                    && message.ends_with(&expected_tail),
                "case {case:?}: expected the refusal to end with {expected_tail:?}, got \
                 {message:?}"
            );
            // Nothing was minted for a caller that was refused.
            assert!(
                answered.get("IceServerList").is_none(),
                "case {case:?}: a refused call must issue no credentials, got {answered}"
            );
        }

        let (status, answered) = harness
            .signaling(
                &key,
                SEND_ALEXA_OFFER_TO_MASTER,
                json!({
                    "ChannelARN": CHANNEL,
                    "SenderClientId": "alexa-1",
                    "MessagePayload": "eA==",
                }),
            )
            .await;
        assert_eq!(
            status, row.offer_status,
            "case {case:?}: SendAlexaOfferToMaster expected HTTP {}, got {status} with \
             {answered}",
            row.offer_status
        );
        if row.offer_status == DENIED_STATUS {
            assert_eq!(
                error_code(&answered),
                DENIED_CODE,
                "case {case:?}: expected {DENIED_CODE}, got {answered}"
            );
        }
    }

    // The account's own published credential holds no IAM policies — the position AWS
    // account root credentials are in — and is served.
    let (status, answered) = harness
        .signaling(
            ACCOUNT_KEY,
            GET_ICE_SERVER_CONFIG,
            json!({ "ChannelARN": CHANNEL }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"the account's own credential\": expected HTTP 200, got {status} with {answered}"
    );

    // A refused caller leaves no record behind: the channel issued no credential set and
    // logged no offer for any of the rows above that were refused.
    let state = harness
        .engine
        .state::<signaling::state::ComAmazonawsKinesisvideosignalingAwsacuitysignalingserviceState>();
    let store = state.lock();
    let elsewhere = store
        .resolve_channel(OTHER_CHANNEL)
        .expect("the other channel is registered");
    assert!(
        elsewhere.ice_credential.is_none() && elsewhere.offers.is_empty(),
        "a channel no permitted call addressed must hold nothing, got {elsewhere:?}"
    );
}
