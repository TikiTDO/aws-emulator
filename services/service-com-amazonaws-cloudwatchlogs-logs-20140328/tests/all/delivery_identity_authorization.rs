// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the IAM AUTHORIZATION of this service's delivery operations.
//!
//! RULE: a delivery operation whose model declares `AccessDeniedException` is decided
//! against the CALLING PRINCIPAL's identity-based IAM policies before this service reads or
//! writes anything — the principal being the identity the access key id the request presents
//! speaks for, and for an `sts:AssumeRole` credential that is the session, whose permissions
//! are the role's. The decision is IAM's own: an explicit `Deny` about the request wins over
//! every `Allow`, an `Allow` with none permits, and a principal with neither is refused. The
//! resource decided against is the one the operation acts on, built from the names the
//! request carried: the delivery SOURCE and the delivery DESTINATION a create pairs
//! ("CreateDelivery … pairing exactly one delivery source and one delivery destination"),
//! and the DELIVERY itself for an update of its configuration. A refusal carries the
//! modelled `AccessDeniedException` naming the principal, the action `logs:<OperationName>`,
//! the resource and the rule that withheld the grant, and the call persists nothing.
//!
//! The rows span what the rule generalises over: which operation is asked for, whether the
//! principal holds no policy / a broad grant / a grant plus an explicit deny / a grant
//! scoped to one resource ARN / a grant scoped to ANOTHER resource ARN, and which of the
//! resources a create names the deciding statement is about. Beside the table: the decision
//! precedes resolving the resource, so a caller holding no grant is refused for a resource
//! the account does not even hold; and the account's own published credential, which holds
//! no IAM policies, keeps being served.
//!
//! The principals are made with IAM's and STS's OWN operations over the same engine, so the
//! documents this service reads are the ones those services wrote, and every case drives the
//! service the way a client does — `POST /` with `X-Amz-Target: Logs_20140328.<Operation>`.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{error_code, REGION};

/// The operations under test, plus the reader a write is proved through.
const CREATE: &str = "CreateDelivery";
const UPDATE: &str = "UpdateDeliveryConfiguration";
const GET: &str = "GetDelivery";
const DESCRIBE: &str = "DescribeDeliveries";

/// The modelled error code of an authorization refusal.
const ACCESS_DENIED: &str = "AccessDeniedException";

/// The error code a request naming a resource this account does not hold is refused with,
/// which a caller the policies DO allow reaches and one they do not never sees.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The whole sentence each of those refusals carries: fixed text for its class, naming no
/// value of the request.
const SOURCE_NOT_FOUND: &str = "Requested Delivery Source does not exist in this account.";
const DESTINATION_NOT_FOUND: &str =
    "Requested Delivery Destination does not exist in this account.";

/// The access-key id standing for the account's own published credential: neither STS nor IAM
/// holds a record for it, so it resolves to the account's ambient identity, which holds no IAM
/// policies — the position AWS account root credentials are in. Every fixture is made with it.
pub(crate) const ACCOUNT_KEY: &str = "AKIDEXAMPLE";

/// The `RoleSessionName` every `AssumeRole` here asks for — the last segment of the
/// assumed-role ARN a refusal names as the principal.
const SESSION: &str = "probe";

/// A principal a call is signed as: the access key id it presents, and the ARN a refusal must
/// blame it by — what `sts:GetCallerIdentity` reports for that key.
pub(crate) struct Principal {
    pub(crate) access_key_id: String,
    pub(crate) arn: String,
}

/// This service beside the IAM and STS services its callers are made with, all on ONE shared
/// engine.
///
/// `crate::harness::Logs` mounts this service alone and signs every call with one key, which
/// is right for a suite about what this service stores; a suite about WHO is calling needs the
/// two services that hold principals and their policies, and a credential scope naming a
/// different key per call, so it assembles the router here. Shared with the suite about the
/// identity decision taken in front of EVERY operation
/// (`crate::identity_policy_authorization`), so one router stands for both.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Nothing here signs: the `Authorization` header carries only the credential scope,
        // which is where a service reads the caller's access key id and Region from.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
                engine,
            ),
        );
        Self { router }
    }

    /// One call to this service, signed for [`REGION`] with `access_key_id` in the credential
    /// scope, returning `(status, parsed body)`.
    pub(crate) async fn call_as(
        &self,
        access_key_id: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        let (status, _headers, parsed) = self.reported_as(access_key_id, operation, body).await;
        (status, parsed)
    }

    /// [`Emulator::call_as`], with the response HEADERS as well — for a rule about what a
    /// response reports beside its body.
    pub(crate) async fn reported_as(
        &self,
        access_key_id: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, http::HeaderMap, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("Logs_20140328.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/logs/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a valid test request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let payload = response.into_body();
        let parsed = if payload.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&payload).unwrap_or(Value::Null)
        };
        (status, headers, parsed)
    }

    /// A call signed with the account's own credential that must succeed.
    pub(crate) async fn ok(&self, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.call_as(ACCOUNT_KEY, operation, body).await;
        assert_eq!(
            status, 200,
            "{operation} {body}: expected HTTP 200, got {status} {parsed}"
        );
        parsed
    }

    /// One awsQuery call to IAM or STS, signed with the account's own credential, which must
    /// succeed.
    pub(crate) async fn query(
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
                    "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/{signing_name}/\
                     aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .body(bytes::Bytes::from(body))
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

/// One identity-based policy document carrying one statement.
pub(crate) fn statement(effect: &str, action: &str, resource: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": effect, "Action": action, "Resource": resource}],
    })
    .to_string()
}

/// Create an IAM role carrying one inline policy per document in `policies`, assume it through
/// STS, and answer the session's access key id with the session ARN.
pub(crate) async fn assumed_role(
    aws: &Emulator,
    role_name: &str,
    policies: &[String],
) -> Principal {
    let account = engine::emulator_account_id();
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"AWS": format!("arn:aws:iam::{account}:root")},
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string();
    aws.query(
        "iam",
        "2010-05-08",
        "CreateRole",
        &[
            ("RoleName", role_name.to_string()),
            ("AssumeRolePolicyDocument", trust),
        ],
    )
    .await;
    // SEVERAL inline policies, so a decision taken over the union of the documents in force
    // is distinguishable from one taken inside a single document.
    for (index, policy) in policies.iter().enumerate() {
        aws.query(
            "iam",
            "2010-05-08",
            "PutRolePolicy",
            &[
                ("RoleName", role_name.to_string()),
                ("PolicyName", format!("inline-{index}")),
                ("PolicyDocument", policy.clone()),
            ],
        )
        .await;
    }
    let assumed = aws
        .query(
            "sts",
            "2011-06-15",
            "AssumeRole",
            &[
                ("RoleArn", format!("arn:aws:iam::{account}:role/{role_name}")),
                ("RoleSessionName", SESSION.to_string()),
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

/// A delivery source over one log group of this account, answering the ARN the source carries.
async fn delivery_source(aws: &Emulator, name: &str) -> String {
    let group = format!("/vended/{name}");
    aws.ok("CreateLogGroup", &json!({"logGroupName": &group}))
        .await;
    let described = aws
        .ok(
            "DescribeLogGroups",
            &json!({"logGroupIdentifiers": [&group]}),
        )
        .await;
    let group_arn = described["logGroups"][0]["logGroupArn"]
        .as_str()
        .unwrap_or_else(|| panic!("a log group carries logGroupArn, got {described}"))
        .to_string();
    let put = aws
        .ok(
            "PutDeliverySource",
            &json!({
                "name": name,
                "resourceArn": group_arn,
                "logType": "INSIGHTS_QUERY_LOGS",
            }),
        )
        .await;
    put["deliverySource"]["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("a put answers the source's ARN, got {put}"))
        .to_string()
}

/// A delivery destination over an S3 bucket, answering the destination's own ARN.
async fn delivery_destination(aws: &Emulator, name: &str) -> String {
    let put = aws
        .ok(
            "PutDeliveryDestination",
            &json!({
                "name": name,
                "deliveryDestinationConfiguration": {
                    "destinationResourceArn": format!("arn:aws:s3:::vended-{name}"),
                },
            }),
        )
        .await;
    put["deliveryDestination"]["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("a put answers the destination's ARN, got {put}"))
        .to_string()
}

/// The sentence IAM refuses an unauthorized call with, which is the message the modelled
/// `AccessDeniedException` carries.
fn not_authorized(principal: &str, action: &str, resource: &str, reason: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} {reason}"
    )
}

/// The clause a refusal carries when NO readable document allows the action.
fn no_grant(action: &str) -> String {
    format!("because no identity-based policy allows the {action} action")
}

/// The clause a refusal carries when a readable document DENIES the action.
const EXPLICIT_DENY: &str = "with an explicit deny in an identity-based policy";

/// The `message` an error body reports.
fn message_of(case: &str, body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: a refusal carries a message, got {body}"))
        .to_string()
}

/// Assert that `answered` is the modelled refusal naming `principal`, `action`, `resource` and
/// `reason`.
fn assert_refused(
    case: &str,
    answered: &(u16, Value),
    principal: &str,
    action: &str,
    resource: &str,
    reason: &str,
) {
    let (status, body) = answered;
    assert_eq!(
        *status, 400,
        "case {case:?}: expected HTTP 400, got {status} {body}"
    );
    let code = error_code(body);
    assert_eq!(
        code, ACCESS_DENIED,
        "case {case:?}: expected {ACCESS_DENIED}, got {code:?} {body}"
    );
    let said = message_of(case, body);
    assert_eq!(
        said,
        not_authorized(principal, action, resource, reason),
        "case {case:?}: expected IAM's refusal sentence, got {said:?}"
    );
}

/// Every delivery the account holds, as `DescribeDeliveries` reports them to the account's own
/// credential.
async fn described(aws: &Emulator) -> Vec<Value> {
    let page = aws.ok(DESCRIBE, &json!({})).await;
    page["deliveries"]
        .as_array()
        .unwrap_or_else(|| panic!("DescribeDeliveries reports a deliveries list, got {page}"))
        .clone()
}

/// What a row expects of the call it makes: served, or refused on one resource for one reason.
enum Expected {
    Served,
    Refused { resource: usize, explicit: bool },
}

/// RULE: `CreateDelivery` is decided against the caller's identity-based policies, on BOTH
/// resources it pairs, before anything is stored.
#[tokio::test]
async fn create_delivery_is_decided_against_the_callers_identity_policies() {
    let action = "logs:CreateDelivery";
    // (case, how the row's policies are built from its own source and destination ARNs, what
    // the call must answer — a refusal naming resource 0 (the source) or 1 (the destination))
    type Policies = fn(&str, &str) -> Vec<String>;
    let rows: Vec<(&str, Policies, Expected)> = vec![
        (
            "a role carrying no policy at all",
            |_source, _destination| Vec::new(),
            Expected::Refused {
                resource: 0,
                explicit: false,
            },
        ),
        (
            "a broad grant over this service",
            |_source, _destination| vec![statement("Allow", "logs:*", "*")],
            Expected::Served,
        ),
        (
            "a broad grant and an explicit deny of this action",
            |_source, _destination| {
                vec![
                    statement("Allow", "logs:*", "*"),
                    statement("Deny", "logs:CreateDelivery", "*"),
                ]
            },
            Expected::Refused {
                resource: 0,
                explicit: true,
            },
        ),
        (
            "a grant scoped to the two resources the request pairs",
            |source, destination| {
                vec![
                    statement("Allow", "logs:CreateDelivery", source),
                    statement("Allow", "logs:CreateDelivery", destination),
                ]
            },
            Expected::Served,
        ),
        (
            "a grant scoped to the source alone",
            |source, _destination| vec![statement("Allow", "logs:CreateDelivery", source)],
            Expected::Refused {
                resource: 1,
                explicit: false,
            },
        ),
        (
            "a broad grant and an explicit deny on the destination alone",
            |_source, destination| {
                vec![
                    statement("Allow", "logs:*", "*"),
                    statement("Deny", "logs:CreateDelivery", destination),
                ]
            },
            Expected::Refused {
                resource: 1,
                explicit: true,
            },
        ),
        (
            "a grant of another action on both resources",
            |source, destination| {
                vec![
                    statement("Allow", "logs:GetDelivery", source),
                    statement("Allow", "logs:GetDelivery", destination),
                ]
            },
            Expected::Refused {
                resource: 0,
                explicit: false,
            },
        ),
        (
            "an explicit deny of every action of this service",
            |_source, _destination| vec![statement("Deny", "logs:*", "*")],
            Expected::Refused {
                resource: 0,
                explicit: true,
            },
        ),
    ];

    for (index, (case, policies, expected)) in rows.iter().enumerate() {
        let aws = Emulator::new();
        let source_name = format!("source-{index}");
        let destination_name = format!("destination-{index}");
        let source_arn = delivery_source(&aws, &source_name).await;
        let destination_arn = delivery_destination(&aws, &destination_name).await;
        let caller = assumed_role(
            &aws,
            &format!("DeliveryRole{index}"),
            &policies(&source_arn, &destination_arn),
        )
        .await;

        let body = json!({
            "deliverySourceName": &source_name,
            "deliveryDestinationArn": &destination_arn,
        });
        let answered = aws.call_as(&caller.access_key_id, CREATE, &body).await;
        match expected {
            Expected::Served => {
                let (status, created) = &answered;
                assert_eq!(
                    *status, 200,
                    "case {case:?}: expected HTTP 200, got {status} {created}"
                );
                let id = created["delivery"]["id"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: a create reports an id, got {created}"))
                    .to_string();
                // The write is proved through a READ: what the create answered is what the
                // store holds afterwards.
                let read = aws.ok(GET, &json!({"id": &id})).await;
                assert_eq!(
                    read["delivery"]["deliverySourceName"].as_str(),
                    Some(source_name.as_str()),
                    "case {case:?}: {GET} must report the created delivery's source, got {read}"
                );
                assert_eq!(
                    read["delivery"]["deliveryDestinationArn"].as_str(),
                    Some(destination_arn.as_str()),
                    "case {case:?}: {GET} must report the destination the create paired, got \
                     {read}"
                );
            }
            Expected::Refused { resource, explicit } => {
                let resource = [&source_arn, &destination_arn][*resource];
                let reason = if *explicit {
                    EXPLICIT_DENY.to_string()
                } else {
                    no_grant(action)
                };
                assert_refused(case, &answered, &caller.arn, action, resource, &reason);
                // A refused create stores nothing: the account holds no delivery at all.
                let held = described(&aws).await;
                assert!(
                    held.is_empty(),
                    "case {case:?}: a refused create must store nothing, got {held:?}"
                );
            }
        }
    }
}

/// RULE: `UpdateDeliveryConfiguration` is decided the same way, on the DELIVERY the request
/// names, and a refused update leaves the stored configuration as it was.
#[tokio::test]
async fn update_delivery_configuration_is_decided_against_the_callers_identity_policies() {
    let action = "logs:UpdateDeliveryConfiguration";
    // (case, how the row's policies are built from the delivery's own ARN, what the call must
    // answer)
    type Policies = fn(&str) -> Vec<String>;
    let rows: Vec<(&str, Policies, Expected)> = vec![
        (
            "a role carrying no policy at all",
            |_delivery| Vec::new(),
            Expected::Refused {
                resource: 0,
                explicit: false,
            },
        ),
        (
            "a broad grant over this service",
            |_delivery| vec![statement("Allow", "logs:*", "*")],
            Expected::Served,
        ),
        (
            "a grant scoped to this delivery",
            |delivery| vec![statement("Allow", "logs:UpdateDeliveryConfiguration", delivery)],
            Expected::Served,
        ),
        (
            "a grant scoped to another delivery",
            |_delivery| {
                vec![statement(
                    "Allow",
                    "logs:UpdateDeliveryConfiguration",
                    "arn:aws:logs:eu-west-2:000000000000:delivery:ffffffffffffffff",
                )]
            },
            Expected::Refused {
                resource: 0,
                explicit: false,
            },
        ),
        (
            "a broad grant and an explicit deny on this delivery",
            |delivery| {
                vec![
                    statement("Allow", "logs:*", "*"),
                    statement("Deny", "logs:UpdateDeliveryConfiguration", delivery),
                ]
            },
            Expected::Refused {
                resource: 0,
                explicit: true,
            },
        ),
    ];

    for (index, (case, policies, expected)) in rows.iter().enumerate() {
        let aws = Emulator::new();
        let source_name = format!("source-{index}");
        let destination_arn = delivery_destination(&aws, &format!("destination-{index}")).await;
        delivery_source(&aws, &source_name).await;
        // The delivery, and its configuration before the update, are made with the account's
        // own credential.
        let created = aws
            .ok(
                CREATE,
                &json!({
                    "deliverySourceName": &source_name,
                    "deliveryDestinationArn": &destination_arn,
                    "fieldDelimiter": "|",
                }),
            )
            .await;
        let id = created["delivery"]["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a create reports an id, got {created}"))
            .to_string();
        let delivery_arn = created["delivery"]["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a create reports an ARN, got {created}"))
            .to_string();
        let caller = assumed_role(
            &aws,
            &format!("UpdateRole{index}"),
            &policies(&delivery_arn),
        )
        .await;

        let body = json!({"id": &id, "fieldDelimiter": ";"});
        let answered = aws.call_as(&caller.access_key_id, UPDATE, &body).await;
        match expected {
            Expected::Served => {
                let (status, said) = &answered;
                assert_eq!(
                    *status, 200,
                    "case {case:?}: expected HTTP 200, got {status} {said}"
                );
            }
            Expected::Refused { explicit, .. } => {
                let reason = if *explicit {
                    EXPLICIT_DENY.to_string()
                } else {
                    no_grant(action)
                };
                assert_refused(case, &answered, &caller.arn, action, &delivery_arn, &reason);
            }
        }
        // What the delivery carries afterwards: the update's value when it was served, and the
        // value the create stored when it was refused.
        let expected_delimiter = match expected {
            Expected::Served => ";",
            Expected::Refused { .. } => "|",
        };
        let read = aws.ok(GET, &json!({"id": &id})).await;
        assert_eq!(
            read["delivery"]["fieldDelimiter"].as_str(),
            Some(expected_delimiter),
            "case {case:?}: expected the delimiter {expected_delimiter:?} after the call, got \
             {read}"
        );
    }
}

/// RULE: the decision comes BEFORE the resource is resolved, so what a caller is told does not
/// depend on what the account holds: a principal holding no grant is refused
/// `AccessDeniedException` for a delivery source that does not exist, where a principal the
/// policies allow reaches this service's own not-found refusal for the same request — the
/// destination's when that is the resource the account does not hold, and the source's once the
/// destination resolves.
#[tokio::test]
async fn the_decision_precedes_resolving_what_the_request_names() {
    let aws = Emulator::new();
    let destination_arn = delivery_destination(&aws, "destination").await;
    let absent = "no-such-source";
    let unauthorized = assumed_role(&aws, "NoGrantRole", &[]).await;
    let authorized = assumed_role(
        &aws,
        "BroadGrantRole",
        &[statement("Allow", "logs:*", "*")],
    )
    .await;

    let body = json!({
        "deliverySourceName": absent,
        "deliveryDestinationArn": &destination_arn,
    });
    let answered = aws
        .call_as(&unauthorized.access_key_id, CREATE, &body)
        .await;
    assert_refused(
        "a caller holding no grant, naming a source the account does not hold",
        &answered,
        &unauthorized.arn,
        "logs:CreateDelivery",
        &format!(
            "arn:aws:logs:{REGION}:{}:delivery-source:{absent}",
            engine::emulator_account_id()
        ),
        &no_grant("logs:CreateDelivery"),
    );

    // (case, the request the allowed caller makes, the sentence it is refused with)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "an allowed caller, naming a source the account does not hold",
            body.clone(),
            SOURCE_NOT_FOUND,
        ),
        (
            "an allowed caller, naming neither resource",
            json!({
                "deliverySourceName": absent,
                "deliveryDestinationArn": format!("{destination_arn}-missing"),
            }),
            DESTINATION_NOT_FOUND,
        ),
    ];
    for (case, request, expected) in rows {
        let (status, answer) = aws
            .call_as(&authorized.access_key_id, CREATE, &request)
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {answer}"
        );
        let code = error_code(&answer);
        assert_eq!(
            code, NOT_FOUND,
            "case {case:?}: expected {NOT_FOUND}, got {code:?} {answer}"
        );
        let said = message_of(case, &answer);
        assert_eq!(
            said, expected,
            "case {case:?}: expected {expected:?}, got {said:?}"
        );
    }
}

/// RULE: the account's own published credential is not an identity this account holds policies
/// for — the position AWS account root credentials are in — so its calls are served whatever a
/// role's policies say. It is what every fixture above is made with.
#[tokio::test]
async fn the_accounts_own_credential_is_not_bound_by_a_roles_policies() {
    let aws = Emulator::new();
    let source_name = "source";
    delivery_source(&aws, source_name).await;
    let destination_arn = delivery_destination(&aws, "destination").await;
    assumed_role(&aws, "DenyAllRole", &[statement("Deny", "logs:*", "*")]).await;

    let created = aws
        .ok(
            CREATE,
            &json!({
                "deliverySourceName": source_name,
                "deliveryDestinationArn": &destination_arn,
            }),
        )
        .await;
    let id = created["delivery"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("a create reports an id, got {created}"))
        .to_string();
    aws.ok(UPDATE, &json!({"id": &id, "fieldDelimiter": ","}))
        .await;
    let read = aws.ok(GET, &json!({"id": &id})).await;
    assert_eq!(
        read["delivery"]["fieldDelimiter"].as_str(),
        Some(","),
        "case \"the account's own credential\": both calls must be served and the update \
         stored, got {read}"
    );
}

/// The action a create needs BESIDES its own when the destination resource is one this
/// service owns: wiring the delivery attaches that resource's own resource policy on the
/// caller's behalf, and a log group is the only resource a policy of this service can be
/// attached to (`PutResourcePolicy$resourceArn`: "Currently only supports LogGroup ARN").
const POLICY_WRITE: &str = "logs:PutResourcePolicy";

/// The whole sentence that refusal carries: fixed text for its class, naming no value of the
/// request — neither the destination, nor the resource behind it, nor the principal, nor the
/// action that was withheld.
const DESTINATION_ACCESS_DENIED: &str = "Access Denied for this Delivery Destination. Please \
                                         make sure that you have correct permissions to \
                                         access the Log Destination Resource.";

/// A delivery destination over `destination_resource`, or a TRACE destination when that is
/// `None` — "deliveryDestinationConfiguration is required for CloudWatch Logs, Amazon S3,
/// Firehose log delivery destinations and not required for X-Ray trace delivery
/// destinations" (`PutDeliveryDestination$deliveryDestinationConfiguration`). Answers the
/// destination's own ARN.
async fn vendor_delivery_destination(
    aws: &Emulator,
    name: &str,
    destination_resource: Option<&str>,
) -> String {
    let body = match destination_resource {
        Some(resource) => json!({
            "name": name,
            "deliveryDestinationConfiguration": {"destinationResourceArn": resource},
        }),
        None => json!({"name": name, "deliveryDestinationType": "XRAY"}),
    };
    let put = aws.ok("PutDeliveryDestination", &body).await;
    put["deliveryDestination"]["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("a put answers the destination's ARN, got {put}"))
        .to_string()
}

/// A delivery destination over a LOG GROUP of this account, answering
/// `(the destination's own ARN, the log group's ARN)` — the log group being the resource the
/// policy that wires a delivery to it is attached to.
async fn log_group_delivery_destination(aws: &Emulator, name: &str) -> (String, String) {
    let group = format!("/aws/vendedlogs/{name}");
    aws.ok("CreateLogGroup", &json!({"logGroupName": &group}))
        .await;
    let described = aws
        .ok(
            "DescribeLogGroups",
            &json!({"logGroupIdentifiers": [&group]}),
        )
        .await;
    let group_arn = described["logGroups"][0]["logGroupArn"]
        .as_str()
        .unwrap_or_else(|| panic!("a log group carries logGroupArn, got {described}"))
        .to_string();
    let destination_arn = vendor_delivery_destination(aws, name, Some(&group_arn)).await;
    (destination_arn, group_arn)
}

/// RULE: a create whose delivery destination represents a resource THIS service owns is
/// decided against the write this service makes on the caller's behalf to wire it — the
/// destination resource's own resource policy — as well as against `logs:CreateDelivery` on
/// the pair, so a principal whose policies do not let it put that resource's policy is
/// refused `AccessDeniedException` with that class's fixed sentence and the create stores
/// nothing.
///
/// The rows span what the rule generalises over: whether the policy write is granted broadly,
/// not granted at all, granted on the destination resource, granted on ANOTHER resource of
/// the same kind, denied broadly, or denied on the destination resource alone.
#[tokio::test]
async fn create_delivery_is_decided_against_the_destination_policy_write() {
    // (case, how the row's policies are built from its own source, destination and log-group
    // ARNs, whether the call must be served)
    type Policies = fn(&str, &str, &str) -> Vec<String>;
    let rows: Vec<(&str, Policies, bool)> = vec![
        (
            "a broad grant over this service",
            |_source, _destination, _group| vec![statement("Allow", "logs:*", "*")],
            true,
        ),
        (
            "a broad grant with an explicit deny of the destination policy write",
            |_source, _destination, _group| {
                vec![
                    statement("Allow", "logs:*", "*"),
                    statement("Deny", POLICY_WRITE, "*"),
                ]
            },
            false,
        ),
        (
            "a grant of the create alone, on the two resources it pairs",
            |source, destination, _group| {
                vec![
                    statement("Allow", "logs:CreateDelivery", source),
                    statement("Allow", "logs:CreateDelivery", destination),
                ]
            },
            false,
        ),
        (
            "the policy write granted on the destination's own log group",
            |_source, _destination, group| {
                vec![
                    statement("Allow", "logs:CreateDelivery", "*"),
                    statement("Allow", POLICY_WRITE, group),
                ]
            },
            true,
        ),
        (
            "the policy write granted on another log group",
            |_source, _destination, _group| {
                vec![
                    statement("Allow", "logs:CreateDelivery", "*"),
                    statement(
                        "Allow",
                        POLICY_WRITE,
                        &format!(
                            "arn:aws:logs:{REGION}:{}:log-group:/aws/vendedlogs/elsewhere",
                            engine::emulator_account_id()
                        ),
                    ),
                ]
            },
            false,
        ),
        (
            "a broad grant with the policy write denied on the destination's log group alone",
            |_source, _destination, group| {
                vec![
                    statement("Allow", "logs:*", "*"),
                    statement("Deny", POLICY_WRITE, group),
                ]
            },
            false,
        ),
    ];

    for (index, (case, policies, served)) in rows.iter().enumerate() {
        let aws = Emulator::new();
        let source_name = format!("source-{index}");
        let source_arn = delivery_source(&aws, &source_name).await;
        let (destination_arn, group_arn) =
            log_group_delivery_destination(&aws, &format!("destination-{index}")).await;
        let caller = assumed_role(
            &aws,
            &format!("PolicyWriteRole{index}"),
            &policies(&source_arn, &destination_arn, &group_arn),
        )
        .await;

        let body = json!({
            "deliverySourceName": &source_name,
            "deliveryDestinationArn": &destination_arn,
        });
        let (status, answer) = aws.call_as(&caller.access_key_id, CREATE, &body).await;
        if *served {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} {answer}"
            );
            let id = answer["delivery"]["id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a create reports an id, got {answer}"))
                .to_string();
            // The write is proved through a READ: the delivery the create answered is the one
            // the store holds afterwards.
            let read = aws.ok(GET, &json!({"id": &id})).await;
            assert_eq!(
                read["delivery"]["deliveryDestinationArn"].as_str(),
                Some(destination_arn.as_str()),
                "case {case:?}: {GET} must report the destination the create paired, got {read}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} {answer}"
            );
            let code = error_code(&answer);
            assert_eq!(
                code, ACCESS_DENIED,
                "case {case:?}: expected {ACCESS_DENIED}, got {code:?} {answer}"
            );
            // The sentence is this class's own fixed text, whichever rule withheld the grant
            // and whatever the request named — including the log group itself.
            let said = message_of(case, &answer);
            assert_eq!(
                said, DESTINATION_ACCESS_DENIED,
                "case {case:?}: expected {DESTINATION_ACCESS_DENIED:?}, got {said:?}"
            );
            assert!(
                !said.contains(&group_arn) && !said.contains(&caller.arn),
                "case {case:?}: the sentence names no value of the request, got {said:?}"
            );
            let held = described(&aws).await;
            assert!(
                held.is_empty(),
                "case {case:?}: a refused create must store nothing, got {held:?}"
            );
        }
    }
}

/// RULE: the policy write is decided only for a destination resource whose policy is THIS
/// service's to attach — a log group. A destination resource another service owns carries no
/// policy of this service's, so a caller denied this service's policy write still has its
/// create served; the grant for such a destination is the owning service's own to write.
///
/// The rows are the destination resources a delivery destination can represent, from
/// `DeliveryDestinationConfiguration$destinationResourceArn` ("That Amazon Web Services
/// destination can be a log group in CloudWatch Logs, an Amazon S3 bucket, or a delivery
/// stream in Firehose") plus the X-Ray trace destination the model's
/// `DeliveryDestinationType` adds, which needs no destination resource at all.
#[tokio::test]
async fn the_destination_policy_write_is_decided_only_for_a_destination_this_service_owns() {
    let account = engine::emulator_account_id();
    // (case, the destination resource the destination represents, whether the create is served)
    let rows: Vec<(&str, Option<String>, bool)> = vec![
        (
            "a log group in CloudWatch Logs",
            Some(format!(
                "arn:aws:logs:{REGION}:{account}:log-group:/aws/vendedlogs/owned"
            )),
            false,
        ),
        (
            "an Amazon S3 bucket",
            Some("arn:aws:s3:::vended-destination".to_string()),
            true,
        ),
        (
            "a delivery stream in Firehose",
            Some(format!(
                "arn:aws:firehose:{REGION}:{account}:deliverystream/vended-destination"
            )),
            true,
        ),
        ("an X-Ray trace destination", None, true),
    ];

    for (index, (case, destination_resource, served)) in rows.iter().enumerate() {
        let aws = Emulator::new();
        let source_name = format!("source-{index}");
        delivery_source(&aws, &source_name).await;
        let destination_arn = vendor_delivery_destination(
            &aws,
            &format!("destination-{index}"),
            destination_resource.as_deref(),
        )
        .await;
        // One principal for every row: allowed every action of this service EXCEPT the
        // destination policy write.
        let caller = assumed_role(
            &aws,
            &format!("NoPolicyWriteRole{index}"),
            &[
                statement("Allow", "logs:*", "*"),
                statement("Deny", POLICY_WRITE, "*"),
            ],
        )
        .await;

        let (status, answer) = aws
            .call_as(
                &caller.access_key_id,
                CREATE,
                &json!({
                    "deliverySourceName": &source_name,
                    "deliveryDestinationArn": &destination_arn,
                }),
            )
            .await;
        if *served {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} {answer}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} {answer}"
            );
            let code = error_code(&answer);
            assert_eq!(
                code, ACCESS_DENIED,
                "case {case:?}: expected {ACCESS_DENIED}, got {code:?} {answer}"
            );
            let said = message_of(case, &answer);
            assert_eq!(
                said, DESTINATION_ACCESS_DENIED,
                "case {case:?}: expected {DESTINATION_ACCESS_DENIED:?}, got {said:?}"
            );
        }
        let held = described(&aws).await;
        assert_eq!(
            held.len(),
            usize::from(*served),
            "case {case:?}: expected {} stored delivery, got {held:?}",
            usize::from(*served)
        );
    }
}
