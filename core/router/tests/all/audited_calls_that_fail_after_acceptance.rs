// SPDX-License-Identifier: CC0-1.0

//! What CloudTrail event history carries for a call a service ACCEPTED and then failed,
//! read back the way a client audits one: `cloudtrail:LookupEvents`.
//!
//! RULE: a service records every call it serves — read-only ones, and ones that failed
//! AFTER being accepted, included — with the operation's own name as `eventName`, the
//! serving service's `eventSource`, `readOnly` from the operation's own semantics, the
//! members the request was dispatched with as `requestParameters`, and the id the CALLER
//! was answered under as `requestID`. A call that failed after acceptance carries the
//! MODELLED exception SHAPE's name as `errorCode` — while the wire keeps that shape's
//! `awsQueryError` code — the message the caller was answered with, byte for byte, as
//! `errorMessage`, and a null `responseElements`; a call the service SERVED carries neither
//! an `errorCode` nor an `errorMessage` key at all. A record of a GLOBAL service is filed
//! in the partition's leader Region whatever Region the caller signed for, and a record of
//! every other service in the Region that served the call.
//!
//! Only a cross-service suite can pin it: a record is written by the service that SERVES
//! the call — IAM over awsQuery, CloudTrail over its own awsJson1_1 front door — and read
//! back through `cloudtrail:LookupEvents`, which reports only the records filed in the
//! Region the lookup is made in. That is two services over ONE engine, as
//! `core/router/src/lib.rs` mounts them. Each service's own per-record shape is pinned by
//! its crate's own suite (`tests/all/management_event_recording.rs` for IAM,
//! `tests/all/event_history_of_every_served_call.rs` for CloudTrail); what is asserted here
//! is the join a client actually makes — the response it holds, and the record of that very
//! call.
//!
//! The rows span what the rule generalises over rather than the one call that motivated it:
//! the OUTCOME (accepted-and-failed, and served), the kind of call (a read of one entity, a
//! listing that names no entity, a create), the ENTITY type the read addressed (a user, a
//! role, a group), the Region the caller signed for, and — on the other side of the filing
//! rule — a non-global service's own read in two Regions that are not the leader.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, form_encode, opt_element};

/// The Region a GLOBAL service's records are filed in, and so the Region a lookup for them
/// is made in (`service::management_events::EVENT_REGION` of the IAM crate).
const LEADER_REGION: &str = "us-east-1";

/// Two Regions that are NOT the leader, for the calls whose records are filed in the Region
/// that served them — and for the IAM calls whose records are filed in the leader Region
/// even so.
const OTHER_REGION: &str = "eu-west-1";
const THIRD_REGION: &str = "ap-southeast-2";

/// The long-term key every emulator publishes: the account's own credential, which every
/// call below is signed with.
const AMBIENT_KEY: &str = "AKIAIOSFODNN7EXAMPLE";

/// The `eventSource` CloudTrail stamps on a record of an IAM call, and on one of its own.
const IAM_EVENT_SOURCE: &str = "iam.amazonaws.com";
const CLOUDTRAIL_EVENT_SOURCE: &str = "cloudtrail.amazonaws.com";

/// The `eventCategory` of a control-plane call's record.
const MANAGEMENT: &str = "Management";

/// The header AWS reports a response's request id in, lowercased as HTTP compares it.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// The user the served rows read, and the one the create row mints.
const HELD_USER: &str = "audited-held-user";
const MINTED_USER: &str = "audited-minted-user";

/// Names the account holds no entity under, for the rows that fail after acceptance.
const ABSENT_USER: &str = "audited-absent-user";
const ABSENT_ROLE: &str = "audited-absent-role";
const ABSENT_GROUP: &str = "audited-absent-group";

/// IAM and CloudTrail over ONE engine, as `core/router/src/lib.rs` mounts them.
///
/// Named for the two services it mounts rather than `build_router`, which is the crate's own
/// whole-router assembly: only the service that WRITES a record and the one that READS it are
/// needed here.
fn iam_and_cloudtrail_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_cloudtrail_cloudtrail_20131101::EmulatedService::new(engine),
    );
    router
}

/// One answer as its caller reads it: the status, the body, and the id the response named
/// the request by — which is the id the record of that call must carry.
struct Answered {
    status: u16,
    body: String,
    request_id: String,
}

/// Dispatch one request to `signing_name`, signed (in the routing sense) for `region`.
///
/// The signature is never checked: this router runs with request authentication off, so
/// what the credential scope carries is the key id and the Region a handler reads.
async fn dispatch(
    router: &MultiServiceRouter,
    signing_name: &str,
    region: &str,
    headers: &[(&str, &str)],
    body: String,
) -> Answered {
    let host = format!("{signing_name}.{region}.amazonaws.com");
    let mut builder = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", host)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={AMBIENT_KEY}/20260101/{region}/{signing_name}/\
                 aws4_request, SignedHeaders=host, Signature=not-verified"
            ),
        );
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(Bytes::from(body))
        .expect("the test request builds");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header_id = response
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);
    let body = String::from_utf8_lossy(response.body()).to_string();
    // The id the caller holds: the header AWS reports it in, or — for a protocol whose
    // success document states it instead — the envelope's own element.
    let request_id = header_id
        .or_else(|| opt_element(&body, "RequestId"))
        .unwrap_or_default();
    Answered {
        status,
        body,
        request_id,
    }
}

/// One IAM action, as an awsQuery form POST signed for `region`.
async fn iam(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    params: &[(&'static str, String)],
) -> Answered {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    dispatch(
        router,
        "iam",
        region,
        &[("content-type", "application/x-www-form-urlencoded")],
        body,
    )
    .await
}

/// One CloudTrail operation, as the awsJson1_1 POST its front door serves, signed for
/// `region`.
async fn cloudtrail(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> Answered {
    dispatch(
        router,
        "cloudtrail",
        region,
        &[
            ("content-type", "application/x-amz-json-1.1"),
            (
                "x-amz-target",
                &format!("CloudTrail_20131101.{operation}"),
            ),
        ],
        body.to_string(),
    )
    .await
}

/// The parsed `CloudTrailEvent` documents `cloudtrail:LookupEvents` reports in `region` for
/// one event name — the way a client reads a record back.
async fn lookup(router: &MultiServiceRouter, region: &str, event_name: &str) -> Vec<Value> {
    let answered = cloudtrail(
        router,
        region,
        "LookupEvents",
        &json!({
            "LookupAttributes": [{
                "AttributeKey": "EventName",
                "AttributeValue": event_name,
            }],
            "MaxResults": 50,
        }),
    )
    .await;
    assert_eq!(
        answered.status, 200,
        "LookupEvents(EventName={event_name}) in {region} must succeed, got {} with {}",
        answered.status, answered.body
    );
    let reported: Value = serde_json::from_str(&answered.body)
        .unwrap_or_else(|error| panic!("{error}: {}", answered.body));
    reported["Events"]
        .as_array()
        .unwrap_or_else(|| panic!("LookupEvents reports an Events list, got {}", answered.body))
        .iter()
        .map(|event| {
            let document = event["CloudTrailEvent"]
                .as_str()
                .unwrap_or_else(|| panic!("an event carries its CloudTrailEvent: {event}"));
            serde_json::from_str(document)
                .unwrap_or_else(|error| panic!("CloudTrailEvent is JSON ({error}): {document}"))
        })
        .collect()
}

/// The ONE record among `records` that answers for the call the caller holds `request_id`
/// for — the join a client makes between a response and the audit record of that very call.
fn record_of(case: &str, records: &[Value], request_id: &str) -> Value {
    assert!(
        !request_id.is_empty(),
        "case {case:?}: the response must name the request it answers"
    );
    let found: Vec<&Value> = records
        .iter()
        .filter(|record| record.get("requestID") == Some(&json!(request_id)))
        .collect();
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one record carrying requestID {request_id:?}, got \
         {records:?}"
    );
    found[0].clone()
}

/// One row of the IAM table: the call to make, what the caller must see, and what the
/// record of that call must carry.
struct IamCase {
    case: &'static str,
    /// The Region the caller signs for. A record of this GLOBAL service is filed in the
    /// leader Region whichever one it is.
    signed_region: &'static str,
    /// The awsQuery `Action`, which is also the `eventName` the record is found under.
    action: &'static str,
    params: Vec<(&'static str, String)>,
    status: u16,
    /// The `<Code>` the caller is answered with: the exception shape's `awsQueryError`
    /// code, which is NOT what the record carries. `None` for a served call.
    wire_code: Option<&'static str>,
    /// The `errorCode` the record carries: the modelled exception SHAPE's own name.
    error_code: Option<&'static str>,
    /// The message the caller must be answered with, worded as IAM words that error class.
    error_message: Option<String>,
    /// The operation's own semantics, which is the record's `readOnly`.
    read_only: bool,
    /// Values the record must carry, each as a pointer into the record.
    recorded: Vec<(&'static str, Value)>,
}

/// The rows: every outcome, kind of call, entity type and signing Region the rule spans.
///
/// The absent-entity messages are IAM's own wording for that error class — the fixed text
/// `The <entity> with name <name> cannot be found.` with the name taken from the request
/// (`smithy_query(query="operation", operation="GetUser")` models the error as
/// `NoSuchEntityException`, "The error message describes the resource").
fn iam_cases() -> Vec<IamCase> {
    vec![
        // The class the finding was filed on: a READ the service accepted and then failed.
        // Signed for a Region that is not the leader, so one row carries both halves of the
        // filing rule.
        IamCase {
            case: "a read of a user nothing created",
            signed_region: OTHER_REGION,
            action: "GetUser",
            params: vec![("UserName", ABSENT_USER.to_string())],
            status: 404,
            wire_code: Some("NoSuchEntity"),
            error_code: Some("NoSuchEntityException"),
            error_message: Some(format!("The user with name {ABSENT_USER} cannot be found.")),
            read_only: true,
            recorded: vec![("/requestParameters/userName", json!(ABSENT_USER))],
        },
        // The SAME operation on an entity that DOES exist: no error keys at all, and the
        // member the request named still recorded.
        IamCase {
            case: "a read of a user the account holds",
            signed_region: LEADER_REGION,
            action: "GetUser",
            params: vec![("UserName", HELD_USER.to_string())],
            status: 200,
            wire_code: None,
            error_code: None,
            error_message: None,
            read_only: true,
            recorded: vec![("/requestParameters/userName", json!(HELD_USER))],
        },
        // A second entity type, so the rule is not read off one operation's member.
        IamCase {
            case: "a read of a role nothing created",
            signed_region: LEADER_REGION,
            action: "GetRole",
            params: vec![("RoleName", ABSENT_ROLE.to_string())],
            status: 404,
            wire_code: Some("NoSuchEntity"),
            error_code: Some("NoSuchEntityException"),
            error_message: Some(format!("The role with name {ABSENT_ROLE} cannot be found.")),
            read_only: true,
            recorded: vec![("/requestParameters/roleName", json!(ABSENT_ROLE))],
        },
        // A third, signed for a third Region.
        IamCase {
            case: "a read of a group nothing created",
            signed_region: THIRD_REGION,
            action: "GetGroup",
            params: vec![("GroupName", ABSENT_GROUP.to_string())],
            status: 404,
            wire_code: Some("NoSuchEntity"),
            error_code: Some("NoSuchEntityException"),
            error_message: Some(format!(
                "The group with name {ABSENT_GROUP} cannot be found."
            )),
            read_only: true,
            recorded: vec![("/requestParameters/groupName", json!(ABSENT_GROUP))],
        },
        // A read whose request names no entity at all: it is recorded as a read too.
        IamCase {
            case: "a listing that names no entity",
            signed_region: LEADER_REGION,
            action: "ListUsers",
            params: Vec::new(),
            status: 200,
            wire_code: None,
            error_code: None,
            error_message: None,
            read_only: true,
            recorded: Vec::new(),
        },
        // The other side of `readOnly`: a call that CHANGES the account, whose record
        // carries what it answered.
        IamCase {
            case: "a create",
            signed_region: OTHER_REGION,
            action: "CreateUser",
            params: vec![("UserName", MINTED_USER.to_string())],
            status: 200,
            wire_code: None,
            error_code: None,
            error_message: None,
            read_only: false,
            recorded: vec![
                ("/requestParameters/userName", json!(MINTED_USER)),
                ("/responseElements/user/userName", json!(MINTED_USER)),
            ],
        },
    ]
}

/// Every record names the call it answers for, whichever side of the rule the row is on:
/// the operation, the service that served it, the Region it is filed in, the instant it was
/// served and the credential it was signed with.
fn assert_common(case: &str, record: &Value, event_name: &str, event_source: &str, region: &str) {
    for (member, expected) in [
        ("eventName", json!(event_name)),
        ("eventSource", json!(event_source)),
        ("eventCategory", json!(MANAGEMENT)),
        ("awsRegion", json!(region)),
    ] {
        assert_eq!(
            record.get(member),
            Some(&expected),
            "case {case:?}: expected {member} {expected}, got {record}"
        );
    }
    // The instant the call was served, in the whole-second ISO 8601 form CloudTrail
    // publishes the member in: `yyyy-MM-dd'T'HH:mm:ss'Z'`, so 20 bytes with no fractional
    // part.
    let at = record
        .get("eventTime")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: the record states when it was served: {record}"));
    assert_eq!(
        (at.len(), at.contains('.'), at.ends_with('Z')),
        (20, false, true),
        "case {case:?}: expected a whole-second ISO 8601 instant, got {at:?}"
    );
    // And the credential the call was presented with, which is how a reader joins a record
    // to the key that made the call.
    assert_eq!(
        record.pointer("/userIdentity/accessKeyId"),
        Some(&json!(AMBIENT_KEY)),
        "case {case:?}: the record names the credential the call was signed with, got {record}"
    );
}

/// RULE: a call a service accepted and then FAILED is audited exactly as a served one is —
/// with the request's own members, the id the caller was answered under, and `readOnly` from
/// the operation's semantics — and carries the modelled exception SHAPE's name as
/// `errorCode` (the wire keeps that shape's `awsQueryError` code), the caller's own message
/// verbatim as `errorMessage` and a null `responseElements`, while a served call carries
/// neither error key at all. A global service's records are filed in the partition's leader
/// Region whatever Region the caller signed for.
#[tokio::test]
async fn a_call_that_failed_after_acceptance_is_audited_with_its_shape_name_and_message() {
    let router = iam_and_cloudtrail_router();
    let created = iam(
        &router,
        LEADER_REGION,
        "CreateUser",
        &[("UserName", HELD_USER.to_string())],
    )
    .await;
    assert_eq!(
        created.status, 200,
        "setup: CreateUser must succeed, got {} with {}",
        created.status, created.body
    );

    for case in iam_cases() {
        let answered = iam(&router, case.signed_region, case.action, &case.params).await;
        assert_eq!(
            answered.status, case.status,
            "case {:?}: expected HTTP {}, got {} with {}",
            case.case, case.status, answered.status, answered.body
        );

        // A record of this GLOBAL service is filed in the leader Region, whichever Region
        // the caller signed for.
        let records = lookup(&router, LEADER_REGION, case.action).await;
        let record = record_of(case.case, &records, &answered.request_id);
        assert_common(
            case.case,
            &record,
            case.action,
            IAM_EVENT_SOURCE,
            LEADER_REGION,
        );
        assert_eq!(
            record.get("readOnly"),
            Some(&json!(case.read_only)),
            "case {:?}: expected readOnly {}, got {record}",
            case.case,
            case.read_only
        );
        // The request DID reach the operation, so the document of the members it carried is
        // recorded — an empty one for a request that named none — whether the operation then
        // served the call or failed it. A call stopped at authorization is what records no
        // document at all.
        assert!(
            record
                .get("requestParameters")
                .is_some_and(Value::is_object),
            "case {:?}: a dispatched call records the parameters it carried, got {record}",
            case.case
        );
        for (pointer, expected) in &case.recorded {
            assert_eq!(
                record.pointer(pointer),
                Some(expected),
                "case {:?}: expected {pointer} to be {expected}, got {record}",
                case.case
            );
        }

        let Some(error_code) = case.error_code else {
            // A SERVED call: no error key at all, not a key carrying an empty value.
            for absent in ["errorCode", "errorMessage"] {
                assert_eq!(
                    record.get(absent),
                    None,
                    "case {:?}: a served call carries no {absent} key, got {record}",
                    case.case
                );
            }
            continue;
        };

        // The audited code is the modelled exception SHAPE's name, and the wire keeps that
        // shape's own `awsQueryError` code: the two are deliberately not the same value.
        let wire_code = case
            .wire_code
            .expect("a row that fails names the code the caller is answered with");
        assert_eq!(
            element(&answered.body, "Code"),
            wire_code,
            "case {:?}: expected the wire code {wire_code:?}, got {}",
            case.case,
            answered.body
        );
        assert_eq!(
            record.get("errorCode"),
            Some(&json!(error_code)),
            "case {:?}: expected errorCode {error_code:?}, got {record}",
            case.case
        );
        assert_ne!(
            record.get("errorCode"),
            Some(&json!(wire_code)),
            "case {:?}: the record names the exception shape, not the wire code, got \
             {record}",
            case.case
        );

        // The message the record carries is the one the caller was answered with, byte for
        // byte, and it is worded as the service words that error class.
        let answered_message = element(&answered.body, "Message");
        let expected_message = case
            .error_message
            .expect("a row that fails names the message the caller is answered with");
        assert_eq!(
            answered_message, expected_message,
            "case {:?}: expected the message {expected_message:?}, got {answered_message:?}",
            case.case
        );
        assert_eq!(
            record.get("errorMessage"),
            Some(&json!(answered_message)),
            "case {:?}: the record carries the caller's own message, got {record}",
            case.case
        );
        // A call that failed answered no values at all.
        assert_eq!(
            record.get("responseElements"),
            Some(&Value::Null),
            "case {:?}: a failed call answered nothing, got {record}",
            case.case
        );
    }
}

/// RULE: a record of a service that is NOT global is filed in the Region that served the
/// call — so a lookup made in that Region reports it and a lookup in another Region does
/// not — and it carries the members the request named, a list member included.
///
/// The rows differ along the Region the read was served in and along the value its list
/// member carried, which is what the filing rule generalises over.
#[tokio::test]
async fn a_non_global_service_s_read_is_filed_under_the_region_that_served_it() {
    let router = iam_and_cloudtrail_router();

    // (case, the Region the read is served in, the token its list member carries)
    let rows: [(&str, &str, &str); 2] = [
        (
            "a read served in one non-leader Region",
            OTHER_REGION,
            "audited-regional-eu",
        ),
        (
            "the same read served in another Region",
            THIRD_REGION,
            "audited-regional-ap",
        ),
    ];

    for (case, region, token) in rows {
        let answered = cloudtrail(
            &router,
            region,
            "DescribeTrails",
            &json!({ "trailNameList": [token] }),
        )
        .await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: DescribeTrails must be served, got {} with {}",
            answered.status, answered.body
        );

        let records = lookup(&router, region, "DescribeTrails").await;
        let record = record_of(case, &records, &answered.request_id);
        assert_common(
            case,
            &record,
            "DescribeTrails",
            CLOUDTRAIL_EVENT_SOURCE,
            region,
        );
        assert_eq!(
            record.get("readOnly"),
            Some(&json!(true)),
            "case {case:?}: a read reports state and changes none of it, got {record}"
        );
        assert_eq!(
            record.pointer("/requestParameters/trailNameList/0"),
            Some(&json!(token)),
            "case {case:?}: the record carries the list the request named, got {record}"
        );
        assert_eq!(
            record.get("errorCode"),
            None,
            "case {case:?}: a served call carries no errorCode key, got {record}"
        );

        // The leader Region holds no record of it: a non-global service's events are filed
        // under the Region that served them.
        let elsewhere = lookup(&router, LEADER_REGION, "DescribeTrails").await;
        assert!(
            !elsewhere
                .iter()
                .any(|other| other.get("requestID") == Some(&json!(answered.request_id))),
            "case {case:?}: a lookup in {LEADER_REGION} reports no record of a call served \
             in {region}, got {elsewhere:?}"
        );
    }

    // And the converse, for the global service mounted beside it: a lookup outside the
    // leader Region reports none of its records.
    let answered = iam(
        &router,
        OTHER_REGION,
        "GetUser",
        &[("UserName", ABSENT_USER.to_string())],
    )
    .await;
    assert_eq!(
        answered.status, 404,
        "setup: the read of an absent user must fail after acceptance, got {} with {}",
        answered.status, answered.body
    );
    let outside = lookup(&router, OTHER_REGION, "GetUser").await;
    assert!(
        outside.is_empty(),
        "case \"a global service's record outside the leader Region\": a lookup in \
         {OTHER_REGION} reports none of them, got {outside:?}"
    );
    let filed = lookup(&router, LEADER_REGION, "GetUser").await;
    let record = record_of(
        "a global service's record in the leader Region",
        &filed,
        &answered.request_id,
    );
    assert_eq!(
        record.get("awsRegion"),
        Some(&json!(LEADER_REGION)),
        "case \"a global service's record in the leader Region\": a call signed for \
         {OTHER_REGION} is still filed in {LEADER_REGION}, got {record}"
    );
}
