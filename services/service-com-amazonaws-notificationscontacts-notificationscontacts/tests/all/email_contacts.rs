// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the email-contact slice of this service.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what
//! is asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: each of these operations answers from the ONE persisted
//! email-contact record, so anything a create or an activation stored is what a later
//! Get or List reports, and anything never stored (or since deleted) is a modelled
//! 404.

use std::collections::BTreeSet;
use std::sync::Arc;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_notificationscontacts_notificationscontacts::ops::types::EMAIL_CONTACT_STATUS_VALUES;
use service_com_amazonaws_notificationscontacts_notificationscontacts::state::{
    self, ACTIVATION_CODE_LEN, CONTACT_ID_LEN, MAX_EMAIL_CONTACTS, STATUS_ACTIVE, STATUS_INACTIVE,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for. The modelled `EmailContactArn` carries
/// an EMPTY region field, so no ARN may mention it.
const REGION: &str = "us-west-2";
/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
pub(crate) const ACCOUNT: &str = "000000000000";

/// One emulator with an empty store, mounted exactly as the router mounts it, plus
/// the engine it was built from — the test reads the activation code out of that
/// store, because the real service only ever delivers it by email.
///
/// `pub(crate)` so a sibling suite in this test target drives the SAME harness rather
/// than growing a second copy of it.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    pub(crate) engine: Arc<engine::InMemoryEngine>,
}

/// This service's signing name, the routing key every request below carries.
pub(crate) const CONTACTS_SIGNING_NAME: &str = "notifications-contacts";

/// The signing name of the sibling User Notifications service, which
/// [`emulator_with_notifications`] mounts beside this one.
pub(crate) const NOTIFICATIONS_SIGNING_NAME: &str = "notifications";

pub(crate) fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_notificationscontacts_notificationscontacts::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    Emulator { router, engine }
}

/// One emulator with this service AND the sibling User Notifications service
/// mounted on the SAME engine, exactly as the production router mounts the two —
/// for a suite whose rule spans both, a contact of this service being a channel
/// of a NotificationConfiguration of that one.
pub(crate) fn emulator_with_notifications() -> Emulator {
    let mut em = emulator();
    em.router.register_service(
        service_com_amazonaws_notifications_notifications::EmulatedService::new(Arc::clone(
            &em.engine,
        )),
    );
    em
}

/// A request signed for `notifications-contacts` in [`REGION`], the signal the router
/// routes on.
fn request(method: &str, uri: &str, body: Option<&Value>) -> http::Request<bytes::Bytes> {
    request_signed_for(CONTACTS_SIGNING_NAME, method, uri, body)
}

/// A request signed for `signing_name` in [`REGION`] — the routing key the router
/// resolves a request to a mounted service by.
fn request_signed_for(
    signing_name: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDCONTACTS/20260101/{REGION}/{signing_name}/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// The WHOLE response of one call — status, headers and raw body bytes — for a suite
/// that asserts on the framing of an answer rather than on a parsed body.
pub(crate) async fn call_http(
    em: &Emulator,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> http::Response<bytes::Bytes> {
    em.router.dispatch(request(method, uri, body)).await
}

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
pub(crate) async fn call(
    em: &Emulator,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    call_signed_for(em, CONTACTS_SIGNING_NAME, method, uri, body).await
}

/// Status and parsed body of one call signed for `signing_name` — the same walk as
/// [`call`], for a suite that drives a service mounted beside this one.
pub(crate) async fn call_signed_for(
    em: &Emulator,
    signing_name: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = em
        .router
        .dispatch(request_signed_for(signing_name, method, uri, body))
        .await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// The modelled error shape a failed call reported.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        // An error `__type` may be namespaced
        // (`com.amazonaws.notificationscontacts#Foo`).
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// Percent-encode a value for use as a single `@httpLabel` path segment, which is
/// what an SDK does: a non-greedy label may not carry a raw `/`.
pub(crate) fn label(value: &str) -> String {
    value.replace('/', "%2F")
}

pub(crate) async fn create(em: &Emulator, body: &Value) -> (u16, Value) {
    call(em, "POST", "/2022-09-19/emailcontacts", Some(body)).await
}

/// Create a contact that must succeed, returning its ARN.
pub(crate) async fn create_ok(em: &Emulator, name: &str, address: &str) -> String {
    let (status, body) = create(em, &json!({ "name": name, "emailAddress": address })).await;
    assert_eq!(
        status, 201,
        "creating {name:?} must answer 201, got {status} / {body:?}"
    );
    body.get("arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("CreateEmailContact must return an arn, got {body:?}"))
        .to_string()
}

pub(crate) async fn get(em: &Emulator, arn: &str) -> (u16, Value) {
    call(em, "GET", &format!("/emailcontacts/{}", label(arn)), None).await
}

/// The `emailContact` structure a successful Get reported.
pub(crate) async fn get_ok(em: &Emulator, arn: &str) -> Value {
    let (status, body) = get(em, arn).await;
    assert_eq!(
        status, 200,
        "GetEmailContact on {arn} must answer 200, got {status} / {body:?}"
    );
    body.get("emailContact")
        .cloned()
        .unwrap_or_else(|| panic!("GetEmailContact must return emailContact, got {body:?}"))
}

pub(crate) async fn send_code(em: &Emulator, arn: &str) -> (u16, Value) {
    call(
        em,
        "POST",
        &format!("/2022-10-31/emailcontacts/{}/activate/send", label(arn)),
        None,
    )
    .await
}

pub(crate) async fn activate(em: &Emulator, arn: &str, code: &str) -> (u16, Value) {
    call(
        em,
        "PUT",
        &format!("/emailcontacts/{}/activate/{}", label(arn), label(code)),
        None,
    )
    .await
}

pub(crate) async fn delete(em: &Emulator, arn: &str) -> (u16, Value) {
    call(em, "DELETE", &format!("/emailcontacts/{}", label(arn)), None).await
}

pub(crate) async fn list(em: &Emulator, query: &str) -> (u16, Value) {
    let uri = if query.is_empty() {
        "/emailcontacts".to_string()
    } else {
        format!("/emailcontacts?{query}")
    };
    call(em, "GET", &uri, None).await
}

/// The activation code the store currently holds for `arn`, which is the only place
/// it exists: the real service mails it to the contact's address.
pub(crate) fn issued_code(em: &Emulator, arn: &str) -> Option<String> {
    state::service_state(&*em.engine)
        .expect("the emulator's own engine must expose this service's store")
        .lock()
        .contact(arn)
        .and_then(|contact| {
            contact
                .activation_token
                .as_ref()
                .map(|token| token.code.clone())
        })
}

/// Whether the store still holds any record at all for `arn`.
fn store_holds(em: &Emulator, arn: &str) -> bool {
    state::service_state(&*em.engine)
        .expect("the emulator's own engine must expose this service's store")
        .lock()
        .contact(arn)
        .is_some()
}

/// Whether `arn` satisfies the modelled `EmailContactArn` pattern
/// `^arn:aws:notifications-contacts::[0-9]{12}:emailcontact/[a-z0-9]{27}$`
/// (`smithy_query(query="shape",shape="EmailContactArn")`), checked field by field
/// so a failure says which field is wrong.
fn arn_is_well_formed(arn: &str) -> bool {
    let Some(rest) = arn.strip_prefix("arn:aws:notifications-contacts::") else {
        return false;
    };
    let Some((account, resource)) = rest.split_once(':') else {
        return false;
    };
    if account.len() != 12 || !account.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let Some(id) = resource.strip_prefix("emailcontact/") else {
        return false;
    };
    id.len() == CONTACT_ID_LEN
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// A `date-time` timestamp as epoch milliseconds, so two of them can be ordered.
///
/// `EmailContact$creationTime`/`$updateTime` carry `@timestampFormat("date-time")`, so
/// the only wire form either of them takes is an RFC 3339 STRING — anything else is a
/// defect, and `email_contact_timestamp_wire_form.rs` is the suite that owns that rule.
pub(crate) fn as_millis(contact: &Value, member: &str) -> i64 {
    match contact.get(member) {
        Some(Value::String(text)) => {
            let parsed = DateTime::from_str(text, Format::DateTime)
                .unwrap_or_else(|e| panic!("{member} {text:?} must parse as a date-time: {e}"));
            parsed.to_millis().expect("a representable instant")
        }
        other => panic!(
            "{member} is modelled @timestampFormat(\"date-time\"), so it must be an RFC 3339 \
             string, got {other:?} in {contact:?}"
        ),
    }
}

/// A well-formed ARN of this service that names nothing: the id is 27 lowercase
/// alphanumerics, so the modelled pattern admits it and only the store can refuse it.
pub(crate) fn unknown_arn(seed: char) -> String {
    format!(
        "arn:aws:notifications-contacts::{ACCOUNT}:emailcontact/{}",
        std::iter::repeat_n(seed, CONTACT_ID_LEN).collect::<String>()
    )
}

#[tokio::test]
async fn a_created_contact_is_read_back_exactly_as_it_was_stored() {
    let em = emulator();

    // Rows span the class the rule generalises over: the shortest and longest values
    // each modelled length admits, the punctuation the name pattern allows, and
    // zero, one and several tags (tags are not reported by `EmailContact`, so what
    // they must not do is disturb the members that are).
    let longest_name = "n".repeat(64);
    let longest_address = format!("{}@example.com", "a".repeat(242));
    assert_eq!(
        longest_address.len(),
        254,
        "the longest row must sit exactly on the modelled 6..254 bound"
    );
    let cases: Vec<(&str, String, String, Option<Value>)> = vec![
        (
            "shortest name and address the model admits",
            "a".to_string(),
            "a@b.co".to_string(),
            None,
        ),
        (
            "longest name and address the model admits",
            longest_name.clone(),
            longest_address.clone(),
            Some(json!({ "team": "infra" })),
        ),
        (
            "name carrying the pattern's punctuation",
            "oncall-primary.v2~1".to_string(),
            "oncall@example.com".to_string(),
            Some(json!({ "team": "infra", "env": "prod", "oncall": "" })),
        ),
        (
            "underscores, digits and a multi-label domain",
            "team_42".to_string(),
            "team+42@mail.sub.example.co.uk".to_string(),
            Some(json!({})),
        ),
    ];

    let mut seen: BTreeSet<String> = BTreeSet::new();
    for (case, name, address, tags) in &cases {
        let mut body = json!({ "name": name, "emailAddress": address });
        if let Some(tags) = tags {
            body["tags"] = tags.clone();
        }
        let (status, answer) = create(&em, &body).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201, got {status} / {answer:?}"
        );
        let arn = answer
            .get("arn")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: no arn in {answer:?}"))
            .to_string();
        assert!(
            arn_is_well_formed(&arn),
            "case {case:?}: expected an EmailContactArn, got {arn:?}"
        );
        assert!(
            seen.insert(arn.clone()),
            "case {case:?}: every create must mint its own arn, got the repeat {arn:?}"
        );

        let contact = get_ok(&em, &arn).await;
        assert_eq!(
            contact.get("arn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: the read-back arn must be the one created, got {contact:?}"
        );
        assert_eq!(
            contact.get("name").and_then(Value::as_str),
            Some(name.as_str()),
            "case {case:?}: expected name {name:?}, got {contact:?}"
        );
        // The create's `emailAddress` is reported as `address`.
        assert_eq!(
            contact.get("address").and_then(Value::as_str),
            Some(address.as_str()),
            "case {case:?}: expected address {address:?}, got {contact:?}"
        );
        assert_eq!(
            contact.get("status").and_then(Value::as_str),
            Some(STATUS_INACTIVE),
            "case {case:?}: a new contact must be {STATUS_INACTIVE}, got {contact:?}"
        );
        assert!(
            EMAIL_CONTACT_STATUS_VALUES.contains(&STATUS_INACTIVE),
            "case {case:?}: {STATUS_INACTIVE} must be an EmailContactStatus value"
        );
        assert_eq!(
            as_millis(&contact, "creationTime"),
            as_millis(&contact, "updateTime"),
            "case {case:?}: nothing has changed the contact yet, got {contact:?}"
        );
        for required in ["arn", "name", "address", "status", "creationTime", "updateTime"] {
            assert!(
                contact.get(required).is_some(),
                "case {case:?}: EmailContact.{required} is @required, got {contact:?}"
            );
        }
    }

    // Two creates that carry identical values are still two contacts, each readable
    // under its own ARN: the ARN, not the name, identifies the record.
    let first = create_ok(&em, "twin", "twin@example.com").await;
    let second = create_ok(&em, "twin", "twin@example.com").await;
    assert_ne!(
        first, second,
        "two creates of the same values must mint two arns"
    );
    for arn in [&first, &second] {
        let contact = get_ok(&em, arn).await;
        assert_eq!(
            contact.get("arn").and_then(Value::as_str),
            Some(arn.as_str()),
            "each twin must read back under its own arn, got {contact:?}"
        );
    }

    // And every contact created above is listed, exactly once each.
    let (status, listed) = list(&em, "").await;
    assert_eq!(status, 200, "ListEmailContacts must answer 200, got {listed:?}");
    let listed_arns: BTreeSet<String> = listed["emailContacts"]
        .as_array()
        .expect("emailContacts is @required")
        .iter()
        .map(|c| c["arn"].as_str().unwrap_or_default().to_string())
        .collect();
    let mut expected = seen.clone();
    expected.insert(first);
    expected.insert(second);
    assert_eq!(
        listed_arns, expected,
        "the list must report exactly the stored contacts"
    );
}

#[tokio::test]
async fn activation_turns_only_on_the_code_this_service_issued() {
    let em = emulator();
    let arn = create_ok(&em, "oncall-primary", "oncall@example.com").await;

    // Before any activation email went out there is no activation token at all, so
    // there is nothing for a code to be compared against: that is the modelled 404,
    // which `activation_token_resource.rs` owns in full.
    assert!(
        issued_code(&em, &arn).is_none(),
        "a fresh contact must carry no activation code"
    );
    let (status, body) = activate(&em, &arn, "abcdefg").await;
    assert_eq!(
        status, 404,
        "activating before any code was issued must answer the modelled 404, got {status} / {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException, got {body:?}"
    );

    let (status, body) = send_code(&em, &arn).await;
    assert_eq!(
        status, 200,
        "SendActivationCode must answer 200, got {status} / {body:?}"
    );
    let code = issued_code(&em, &arn).expect("SendActivationCode must persist a code");
    assert_eq!(
        code.len(),
        ACTIVATION_CODE_LEN,
        "the issued code must be {ACTIVATION_CODE_LEN} characters, got {code:?}"
    );
    assert!(
        code.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()),
        "the issued code must satisfy ^[a-z0-9]{{7}}$, got {code:?}"
    );

    let before = get_ok(&em, &arn).await;

    // Rows span the wrong-code class: every one is a value the modelled `Token`
    // admits, so only the stored code can tell them apart.
    let wrong_codes: Vec<String> = ["zzzzzzz", "0000000", "abc1234", "9z8y7x6"]
        .iter()
        .map(|c| (*c).to_string())
        .filter(|c| *c != code)
        .collect();
    assert!(
        wrong_codes.len() >= 3,
        "the wrong-code table must keep several rows, got {wrong_codes:?}"
    );
    for wrong in &wrong_codes {
        let (status, body) = activate(&em, &arn, wrong).await;
        assert_eq!(
            status, 400,
            "case {wrong:?}: a code that is not the issued one must be refused, got {status} / {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {wrong:?}: expected ValidationException, got {body:?}"
        );
        let after = get_ok(&em, &arn).await;
        assert_eq!(
            after.get("status").and_then(Value::as_str),
            Some(STATUS_INACTIVE),
            "case {wrong:?}: a refused activation must leave the contact {STATUS_INACTIVE}, got {after:?}"
        );
        assert_eq!(
            as_millis(&after, "updateTime"),
            as_millis(&before, "updateTime"),
            "case {wrong:?}: a refused activation must not move updateTime, got {after:?}"
        );
    }

    // The issued code activates the contact, and the change is what every later read
    // reports.
    let (status, body) = activate(&em, &arn, &code).await;
    assert_eq!(
        status, 200,
        "the issued code must activate the contact, got {status} / {body:?}"
    );
    let active = get_ok(&em, &arn).await;
    assert_eq!(
        active.get("status").and_then(Value::as_str),
        Some(STATUS_ACTIVE),
        "the activated contact must read back {STATUS_ACTIVE}, got {active:?}"
    );
    assert!(
        EMAIL_CONTACT_STATUS_VALUES.contains(&STATUS_ACTIVE),
        "{STATUS_ACTIVE} must be an EmailContactStatus value"
    );
    assert_eq!(
        as_millis(&active, "creationTime"),
        as_millis(&before, "creationTime"),
        "activation must not move creationTime, got {active:?}"
    );
    assert!(
        as_millis(&active, "updateTime") > as_millis(&before, "updateTime"),
        "activation must move updateTime, got {active:?} after {before:?}"
    );
    let listed = list(&em, "").await.1;
    assert_eq!(
        listed["emailContacts"][0]["status"].as_str(),
        Some(STATUS_ACTIVE),
        "the list must report the activated status too, got {listed:?}"
    );

    // Idempotent: the same call again changes nothing and still answers 200.
    let (status, body) = activate(&em, &arn, &code).await;
    assert_eq!(
        status, 200,
        "re-activating with the same code must stay 200, got {status} / {body:?}"
    );
    let again = get_ok(&em, &arn).await;
    assert_eq!(
        (
            again.get("status").and_then(Value::as_str),
            as_millis(&again, "updateTime")
        ),
        (Some(STATUS_ACTIVE), as_millis(&active, "updateTime")),
        "a repeated activation must change nothing, got {again:?} after {active:?}"
    );

    // An activated contact has nothing left to activate, so there is no activation
    // email to send for it.
    let (status, body) = send_code(&em, &arn).await;
    assert_eq!(
        status, 409,
        "SendActivationCode on an active contact must conflict, got {status} / {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ConflictException",
        "expected ConflictException, got {body:?}"
    );

    // Sending another activation email supersedes the previous code: the one
    // outstanding code is the last one issued.
    let other = create_ok(&em, "secondary", "secondary@example.com").await;
    assert_eq!(send_code(&em, &other).await.0, 200, "the first send must succeed");
    let first_code = issued_code(&em, &other).expect("a code was issued");
    assert_eq!(send_code(&em, &other).await.0, 200, "a second send must succeed");
    let second_code = issued_code(&em, &other).expect("a code was issued");
    if first_code != second_code {
        let (status, body) = activate(&em, &other, &first_code).await;
        assert_eq!(
            status, 400,
            "a superseded code must no longer activate, got {status} / {body:?}"
        );
    }
    assert_eq!(
        activate(&em, &other, &second_code).await.0,
        200,
        "the latest issued code must activate the contact"
    );

    // One contact's code is not another's: each code belongs to its own record.
    let third = create_ok(&em, "tertiary", "tertiary@example.com").await;
    assert_eq!(send_code(&em, &third).await.0, 200, "the send must succeed");
    let third_code = issued_code(&em, &third).expect("a code was issued");
    if third_code != second_code {
        let (status, body) = activate(&em, &third, &second_code).await;
        assert_eq!(
            status, 400,
            "another contact's code must not activate this one, got {status} / {body:?}"
        );
    }
}

#[tokio::test]
async fn the_list_pages_over_every_stored_contact_exactly_once() {
    // Rows differ along the two things pagination generalises over: how many
    // contacts the store holds, and how large a page the caller asked for —
    // including a size that divides the total exactly, one that does not, the
    // modelled bounds of `maxResults`, and an empty store.
    let cases: Vec<(&str, usize, Option<i32>)> = vec![
        ("an empty store, no page size", 0, None),
        ("an empty store, page size 1", 0, Some(1)),
        ("one contact, no page size", 1, None),
        ("four contacts, page size 4 divides exactly", 4, Some(4)),
        ("five contacts, page size 2", 5, Some(2)),
        ("three contacts, page size 1", 3, Some(1)),
        ("seven contacts, page size 3", 7, Some(3)),
        ("two contacts, page size at the modelled maximum", 2, Some(100)),
    ];

    for (case, total, page_size) in cases {
        let em = emulator();
        let mut expected: BTreeSet<String> = BTreeSet::new();
        for n in 0..total {
            expected.insert(
                create_ok(
                    &em,
                    &format!("contact-{n:02}"),
                    &format!("contact-{n:02}@example.com"),
                )
                .await,
            );
        }

        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut query = page_size.map(|n| format!("maxResults={n}")).unwrap_or_default();
            if let Some(token) = &token {
                if !query.is_empty() {
                    query.push('&');
                }
                query.push_str(&format!("nextToken={token}"));
            }
            let (status, body) = list(&em, &query).await;
            assert_eq!(
                status, 200,
                "case {case:?}: ListEmailContacts must answer 200, got {status} / {body:?}"
            );
            let page = body["emailContacts"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: emailContacts is @required, got {body:?}"))
                .clone();
            if let Some(size) = page_size {
                assert!(
                    page.len() <= size as usize,
                    "case {case:?}: a page must hold at most {size} contacts, got {}",
                    page.len()
                );
            }
            for contact in &page {
                for required in ["arn", "name", "address", "status", "creationTime", "updateTime"] {
                    assert!(
                        contact.get(required).is_some(),
                        "case {case:?}: EmailContact.{required} is @required, got {contact:?}"
                    );
                }
                collected.push(contact["arn"].as_str().unwrap_or_default().to_string());
            }
            pages += 1;
            token = body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                pages <= total + 1,
                "case {case:?}: paging must terminate, stopped after {pages} pages"
            );
        }

        let unique: BTreeSet<String> = collected.iter().cloned().collect();
        assert_eq!(
            collected.len(),
            unique.len(),
            "case {case:?}: paging must not repeat a contact, got {collected:?}"
        );
        assert_eq!(
            unique, expected,
            "case {case:?}: paging must visit every stored contact exactly once"
        );
        let mut sorted = collected.clone();
        sorted.sort();
        assert_eq!(
            collected, sorted,
            "case {case:?}: the pages must walk one stable total order, got {collected:?}"
        );
        if let Some(size) = page_size {
            let want_pages = total.div_ceil(size as usize).max(1);
            assert_eq!(
                pages, want_pages,
                "case {case:?}: expected {want_pages} pages for {total} contacts"
            );
        }
    }
}

#[tokio::test]
async fn a_page_token_the_list_never_issued_is_refused() {
    let em = emulator();
    for n in 0..3 {
        create_ok(
            &em,
            &format!("contact-{n}"),
            &format!("contact-{n}@example.com"),
        )
        .await;
    }

    // Rows: text that is not hex at all, hex that decodes to something that is not
    // one of this service's ARNs, and hex naming an ARN of another service.
    let forged = [
        ("not hex at all", "not-a-token".to_string()),
        ("hex of a non-arn", hex::encode("just-some-text")),
        (
            "hex of another service's arn",
            hex::encode(format!("arn:aws:aiops::{ACCOUNT}:investigation-group/abcdefghij123456")),
        ),
        (
            "hex of an arn whose id is too short",
            hex::encode(format!("arn:aws:notifications-contacts::{ACCOUNT}:emailcontact/abc")),
        ),
    ];
    for (case, token) in forged {
        let (status, body) = list(&em, &format!("nextToken={token}")).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a forged nextToken must be refused, got {status} / {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body:?}"
        );
    }

    // The modelled `maxResults` range (1..100) is refused at the boundary, not by
    // this handler: each published bound, and one step past it.
    for (case, query, want_status) in [
        ("maxResults below the low bound", "maxResults=0", 400u16),
        ("maxResults at the low bound", "maxResults=1", 200),
        ("maxResults at the high bound", "maxResults=100", 200),
        ("maxResults above the high bound", "maxResults=101", 400),
    ] {
        let (status, body) = list(&em, query).await;
        assert_eq!(
            status, want_status,
            "case {case:?}: expected {want_status}, got {status} / {body:?}"
        );
    }
}

#[tokio::test]
async fn deleting_a_contact_removes_it_from_every_later_read() {
    let em = emulator();
    let mut arns: Vec<String> = Vec::new();
    for n in 0..3 {
        arns.push(
            create_ok(
                &em,
                &format!("doomed-{n}"),
                &format!("doomed-{n}@example.com"),
            )
            .await,
        );
    }
    // One of them carries tags and an outstanding activation code, so the delete has
    // more than the bare record to reclaim.
    let (status, body) = create(
        &em,
        &json!({
            "name": "doomed-tagged",
            "emailAddress": "doomed-tagged@example.com",
            "tags": { "team": "infra" },
        }),
    )
    .await;
    assert_eq!(status, 201, "the tagged create must succeed, got {body:?}");
    let tagged = body["arn"].as_str().expect("an arn").to_string();
    assert_eq!(send_code(&em, &tagged).await.0, 200, "the send must succeed");
    arns.push(tagged);

    for arn in &arns {
        let (status, body) = delete(&em, arn).await;
        assert_eq!(
            status, 200,
            "case {arn}: DeleteEmailContact must answer 200, got {status} / {body:?}"
        );

        let (status, body) = get(&em, arn).await;
        assert_eq!(
            status, 404,
            "case {arn}: a deleted contact must be a 404, got {status} / {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {arn}: expected ResourceNotFoundException, got {body:?}"
        );
        assert!(
            !store_holds(&em, arn),
            "case {arn}: the whole record — tags and activation code included — must be gone"
        );

        let listed = list(&em, "").await.1;
        let still_listed: Vec<&str> = listed["emailContacts"]
            .as_array()
            .expect("emailContacts is @required")
            .iter()
            .map(|c| c["arn"].as_str().unwrap_or_default())
            .collect();
        assert!(
            !still_listed.contains(&arn.as_str()),
            "case {arn}: a deleted contact must not be listed, got {still_listed:?}"
        );

        // Deleting it again has nothing to delete.
        let (status, body) = delete(&em, arn).await;
        assert_eq!(
            status, 404,
            "case {arn}: a second delete must be a 404, got {status} / {body:?}"
        );
    }

    let listed = list(&em, "").await.1;
    assert_eq!(
        listed["emailContacts"].as_array().map(Vec::len),
        Some(0),
        "every contact was deleted, so the list must be empty, got {listed:?}"
    );
}

#[tokio::test]
async fn an_arn_naming_nothing_is_a_modelled_404_not_a_hollow_200() {
    let em = emulator();
    // A real contact exists throughout, so a 404 can only come from the lookup
    // failing — not from an empty store.
    let existing = create_ok(&em, "present", "present@example.com").await;
    assert_eq!(send_code(&em, &existing).await.0, 200, "the send must succeed");
    let code = issued_code(&em, &existing).expect("a code was issued");

    // Rows: every operation that addresses a contact by ARN, each given a
    // well-formed ARN the store does not hold.
    for (case, method, uri) in [
        (
            "GetEmailContact",
            "GET",
            format!("/emailcontacts/{}", label(&unknown_arn('a'))),
        ),
        (
            "DeleteEmailContact",
            "DELETE",
            format!("/emailcontacts/{}", label(&unknown_arn('b'))),
        ),
        (
            "SendActivationCode",
            "POST",
            format!(
                "/2022-10-31/emailcontacts/{}/activate/send",
                label(&unknown_arn('c'))
            ),
        ),
        (
            "ActivateEmailContact",
            "PUT",
            format!(
                "/emailcontacts/{}/activate/{code}",
                label(&unknown_arn('d'))
            ),
        ),
    ] {
        let (status, body) = call(&em, method, &uri, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: an unknown arn must be a 404, got {status} / {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body:?}"
        );
    }

    // An ARN outside the modelled pattern is refused at the boundary instead, with
    // the 400 that says the request itself is wrong.
    for (case, arn) in [
        ("an id shorter than 27 characters", format!("arn:aws:notifications-contacts::{ACCOUNT}:emailcontact/abc")),
        ("an id carrying uppercase", format!("arn:aws:notifications-contacts::{ACCOUNT}:emailcontact/ABCDEFGHIJKLMNOPQRSTUVWXYZ1")),
        ("another service's arn", format!("arn:aws:aiops::{ACCOUNT}:investigation-group/abcdefghij123456")),
        ("not an arn at all", "just-a-name".to_string()),
    ] {
        let (status, body) = get(&em, &arn).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a malformed arn must be refused, got {status} / {body:?}"
        );
    }

    // The contact that does exist is untouched by any of it.
    let contact = get_ok(&em, &existing).await;
    assert_eq!(
        contact.get("status").and_then(Value::as_str),
        Some(STATUS_INACTIVE),
        "the existing contact must be unchanged, got {contact:?}"
    );
    assert_eq!(
        issued_code(&em, &existing).as_deref(),
        Some(code.as_str()),
        "the existing contact must still hold its own activation code"
    );
}

#[tokio::test]
async fn the_store_refuses_to_grow_past_its_cap() {
    let em = emulator();

    // Fill the store to its cap, then one more: the create is refused with the
    // modelled quota error rather than growing the store further.
    for n in 0..MAX_EMAIL_CONTACTS {
        let (status, body) = create(
            &em,
            &json!({
                "name": format!("filler-{n:04}"),
                "emailAddress": format!("filler-{n:04}@example.com"),
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case fill {n}: expected 201 while under the cap, got {status} / {body:?}"
        );
    }

    let (status, body) = create(
        &em,
        &json!({ "name": "one-too-many", "emailAddress": "one-too-many@example.com" }),
    )
    .await;
    assert_eq!(
        status, 402,
        "a create past the cap must be refused, got {status} / {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ServiceQuotaExceededException",
        "a create past the cap must be a ServiceQuotaExceededException, got {body:?}"
    );

    // Freeing one slot lets exactly one more create through.
    let victim = list(&em, "maxResults=1").await.1["emailContacts"][0]["arn"]
        .as_str()
        .expect("a stored contact")
        .to_string();
    assert_eq!(delete(&em, &victim).await.0, 200, "the delete must succeed");
    let (status, body) = create(
        &em,
        &json!({ "name": "one-more", "emailAddress": "one-more@example.com" }),
    )
    .await;
    assert_eq!(
        status, 201,
        "a create into the freed slot must succeed, got {status} / {body:?}"
    );
    let (status, body) = create(
        &em,
        &json!({ "name": "one-too-many", "emailAddress": "one-too-many@example.com" }),
    )
    .await;
    assert_eq!(
        status, 402,
        "the store is full again, got {status} / {body:?}"
    );
}

#[tokio::test]
async fn the_tagging_operations_persist_through_the_same_record() {
    let em = emulator();
    let arn = create_ok(&em, "tagged", "tagged@example.com").await;

    // The whole tagging slice is implemented now: `TagResource` and `UntagResource`
    // write the tag set on the record and `ListTagsForResource` reports it, so none of
    // them answers the scaffold's 501 any more. The rules they follow are owned by
    // `tests/all/resource_tags.rs` (the read), `tests/all/resource_tag_writes.rs` (the
    // writes) and `tests/all/tag_mutation_success_status.rs` (the bodyless success
    // status the two mutations answer); this case only pins that each one is reachable
    // on the wire.
    for (case, method, uri, body, expected) in [
        (
            "TagResource",
            "POST",
            format!("/tags/{}", label(&arn)),
            Some(json!({ "tags": { "team": "infra" } })),
            crate::resource_tag_writes::TAG_MUTATION_SUCCESS_STATUS,
        ),
        (
            "ListTagsForResource",
            "GET",
            format!("/tags/{}", label(&arn)),
            None,
            200,
        ),
        (
            "UntagResource",
            "DELETE",
            format!("/tags/{}?tagKeys=team", label(&arn)),
            None,
            crate::resource_tag_writes::TAG_MUTATION_SUCCESS_STATUS,
        ),
    ] {
        let (status, answer) = call(&em, method, &uri, body.as_ref()).await;
        assert_eq!(
            status, expected,
            "case {case:?}: an implemented operation must answer {expected}, got {status} / {answer:?}"
        );
    }
}
