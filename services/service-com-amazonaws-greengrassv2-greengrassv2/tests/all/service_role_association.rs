// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Greengrass service role an account associates with IoT
//! Greengrass.
//!
//! The rule under test: the service role is ONE association per account — associating a
//! role replaces whatever was associated before and stamps the association with the
//! instant of that call, reading reports the association that is stored (and nothing when
//! none is), and disassociating removes it; every instant reported is the RFC 3339 UTC
//! form, and the instant a read reports is the one the write answered.
//!
//! Every assertion drives the real `restJson1` front door: the request is built the way
//! an SDK builds it, dispatched through [`MultiServiceRouter`], and judged on the HTTP
//! status and the JSON body. Nothing reaches into the state struct, so an association
//! that was reported but never persisted fails here.
//!
//! The rows differ along what the rule generalises over — the FORM of the role ARN
//! associated, the order the three calls are made in, and how many times each is repeated
//! — not along names the rule says nothing about.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::Value;
use service_com_amazonaws_greengrassv2_greengrassv2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region these requests are signed for.
const REGION: &str = "us-west-2";

/// The account id every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// The route all three operations are bound to (`PUT`, `GET` and `DELETE` of
/// `smithy_query(query="operation",operation="AssociateServiceRoleToAccount")` and its two
/// peers).
const SERVICE_ROLE_PATH: &str = "/greengrass/servicerole";

/// The emulator front door with only this service mounted, over ONE shared engine —
/// exactly how `core/router` assembles it.
///
/// This suite owns the service-role harness for the crate: the read-side suite
/// `tests/all/service_role_read_is_a_pure_report.rs` drives these same helpers, so a
/// service-role request is built and read exactly one way here.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Dispatch one signed request and return its status and parsed JSON body.
async fn call(router: &MultiServiceRouter, method: &str, body: Vec<u8>) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://greengrass.{REGION}.amazonaws.com{SERVICE_ROLE_PATH}"
        ))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/greengrass/aws4_request, \
                 SignedHeaders=host, Signature=00"
            ),
        )
        .header("content-type", "application/json")
        .body(Bytes::from(body))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or(Value::String(String::from_utf8_lossy(&raw).into_owned()))
    };
    (status, value)
}

/// `AssociateServiceRoleToAccount` for `role_arn`.
pub(crate) async fn associate(router: &MultiServiceRouter, role_arn: &str) -> (u16, Value) {
    let body = serde_json::to_vec(&serde_json::json!({ "RoleArn": role_arn }))
        .expect("a serializable request body");
    call(router, "PUT", body).await
}

/// `GetServiceRoleForAccount`.
pub(crate) async fn get_role(router: &MultiServiceRouter) -> (u16, Value) {
    call(router, "GET", Vec::new()).await
}

/// `DisassociateServiceRoleFromAccount`.
pub(crate) async fn disassociate(router: &MultiServiceRouter) -> (u16, Value) {
    call(router, "DELETE", Vec::new()).await
}

/// A member of a JSON object, or `Value::Null` when the body carries none.
fn member(body: &Value, key: &str) -> Value {
    body.get(key).cloned().unwrap_or(Value::Null)
}

/// A string member of a JSON object, or the empty string when absent.
pub(crate) fn text(body: &Value, key: &str) -> String {
    body.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The current instant in whole epoch seconds, which is the resolution a reported
/// instant carries.
pub(crate) fn now_seconds() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    )
    .unwrap_or(i64::MAX)
}

/// Assert that `rendered` is an instant in the RFC 3339 UTC form, and that it falls in
/// the window `[opened, closed]` the calling test measured around the call — so the
/// instant is a reading of the clock rather than a fabricated or echoed value. Answers
/// the instant in epoch seconds.
pub(crate) fn instant_within(case: &str, rendered: &str, opened: i64, closed: i64) -> i64 {
    let parsed = DateTime::from_str(rendered, Format::DateTime).unwrap_or_else(|error| {
        panic!("case {case:?}: expected an RFC 3339 instant, got {rendered:?} ({error})")
    });
    // The canonical rendering of the parsed instant must be the text itself: that pins
    // the UTC zone and the date/time layout, not merely that something parsed.
    let canonical = parsed
        .fmt(Format::DateTime)
        .expect("a parsed instant renders back");
    assert_eq!(
        canonical, rendered,
        "case {case:?}: expected the canonical RFC 3339 UTC form, got {rendered:?}"
    );
    let seconds = parsed.secs();
    assert!(
        (opened..=closed).contains(&seconds),
        "case {case:?}: expected an instant in [{opened}, {closed}], got {rendered:?} \
         ({seconds})"
    );
    seconds
}

/// Role ARNs spanning the forms an IAM role ARN takes: the model targets `roleArn` at a
/// plain `String` with no pattern or length constraint
/// (`smithy_query(query="operation",operation="AssociateServiceRoleToAccount")`), so what
/// varies here is the ARN itself — a role at the root path, one under the `service-role`
/// path AWS files a console-created service role under, a service-linked role under its
/// `aws-service-role` path, a long role name (the IAM role-name limit is 64 characters),
/// and a role in another partition and region, which an account's association is not
/// keyed by.
pub(crate) fn role_arn_rows() -> Vec<(&'static str, String)> {
    vec![
        (
            "a role at the root path",
            format!("arn:aws:iam::{ACCOUNT}:role/GreengrassV2TokenExchangeRole"),
        ),
        (
            "a role under the service-role path",
            format!("arn:aws:iam::{ACCOUNT}:role/service-role/Greengrass_ServiceRole"),
        ),
        (
            "a service-linked role",
            format!(
                "arn:aws:iam::{ACCOUNT}:role/aws-service-role/greengrass.amazonaws.com/\
                 AWSServiceRoleForGreengrass"
            ),
        ),
        (
            "a role whose name is as long as IAM admits",
            format!("arn:aws:iam::{ACCOUNT}:role/{}", "R".repeat(64)),
        ),
        (
            "a role in another partition and region",
            "arn:aws-us-gov:iam::123456789012:role/GovGreengrassServiceRole".to_string(),
        ),
    ]
}

#[tokio::test]
async fn associating_a_service_role_replaces_the_one_the_account_holds() {
    let router = emulator();

    // Each row associates over the association the previous row left, so the rule under
    // test is that the write REPLACES rather than adds: after every row the account
    // holds exactly the role that row named, and the instant it reports advances with
    // the calls rather than staying at the first one.
    let mut previous_instant = 0_i64;
    for (case, role_arn) in role_arn_rows() {
        let opened = now_seconds();
        let (status, written) = associate(&router, &role_arn).await;
        let closed = now_seconds();
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from AssociateServiceRoleToAccount, got {status} \
             {written}"
        );
        let associated_at = text(&written, "AssociatedAt");
        let stamped = instant_within(case, &associated_at, opened, closed);
        assert!(
            stamped >= previous_instant,
            "case {case:?}: expected an instant no earlier than the previous call's \
             {previous_instant}, got {stamped}"
        );
        previous_instant = stamped;

        // The read reports the association that was written — the role named by THIS
        // row, and the very instant the write answered.
        for read in 1..=2 {
            let (status, got) = get_role(&router).await;
            assert_eq!(
                status, 200,
                "case {case:?} read {read}: expected 200 from GetServiceRoleForAccount, \
                 got {status} {got}"
            );
            assert_eq!(
                text(&got, "RoleArn"),
                role_arn,
                "case {case:?} read {read}: expected the role just associated, got {got}"
            );
            assert_eq!(
                text(&got, "AssociatedAt"),
                associated_at,
                "case {case:?} read {read}: expected the instant the write answered, \
                 got {got}"
            );
        }
    }

    // Re-associating the SAME role is a replacement too: it is accepted and re-stamps
    // the association, so the read still reports one role and the instant it carries.
    let (case, role_arn) = role_arn_rows()
        .pop()
        .expect("the row catalog is not empty");
    let opened = now_seconds();
    let (status, written) = associate(&router, &role_arn).await;
    let closed = now_seconds();
    assert_eq!(
        status, 200,
        "case {case:?} re-associated: expected 200, got {status} {written}"
    );
    let associated_at = text(&written, "AssociatedAt");
    instant_within("the same role re-associated", &associated_at, opened, closed);
    let (status, got) = get_role(&router).await;
    assert_eq!(
        (status, text(&got, "RoleArn").as_str(), text(&got, "AssociatedAt").as_str()),
        (200, role_arn.as_str(), associated_at.as_str()),
        "case {case:?} re-associated: expected the re-stamped association, got {status} {got}"
    );

    // Disassociating removes that one association, so the account holds none again.
    let (status, removed) = disassociate(&router).await;
    assert_eq!(
        status, 200,
        "disassociating must answer 200, got {status} {removed}"
    );
    let (status, got) = get_role(&router).await;
    assert_eq!(
        (status, member(&got, "RoleArn")),
        (200, Value::Null),
        "after disassociating, the account must hold no service role, got {status} {got}"
    );
}

#[tokio::test]
async fn an_account_with_no_service_role_reports_none_and_disassociating_is_no_failure() {
    let router = emulator();

    // An account starts with no service role associated: the read succeeds and names
    // neither a role nor an instant, because there is no association to report.
    for read in 1..=2 {
        let (status, got) = get_role(&router).await;
        assert_eq!(
            status, 200,
            "case read {read} with nothing associated: expected 200, got {status} {got}"
        );
        for absent in ["RoleArn", "AssociatedAt"] {
            assert_eq!(
                member(&got, absent),
                Value::Null,
                "case read {read} with nothing associated: expected no {absent}, got {got}"
            );
        }
    }

    // Disassociating when there is nothing associated asks for the state the account is
    // already in: that is a success, and nothing was disassociated, so no instant is
    // reported for it.
    for attempt in 1..=2 {
        let (status, removed) = disassociate(&router).await;
        assert_eq!(
            status, 200,
            "case disassociate attempt {attempt} with nothing associated: expected 200, \
             got {status} {removed}"
        );
        assert_eq!(
            member(&removed, "DisassociatedAt"),
            Value::Null,
            "case disassociate attempt {attempt} with nothing associated: nothing was \
             disassociated, so no instant is reported, got {removed}"
        );
    }

    // With a role associated, the disassociation reports the instant it happened, and
    // the account is left holding none — whichever role was associated.
    for (case, role_arn) in role_arn_rows() {
        let (status, written) = associate(&router, &role_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from AssociateServiceRoleToAccount, got {status} \
             {written}"
        );

        let opened = now_seconds();
        let (status, removed) = disassociate(&router).await;
        let closed = now_seconds();
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from DisassociateServiceRoleFromAccount, got \
             {status} {removed}"
        );
        instant_within(case, &text(&removed, "DisassociatedAt"), opened, closed);

        let (status, got) = get_role(&router).await;
        assert_eq!(
            status, 200,
            "case {case:?} after disassociating: expected 200, got {status} {got}"
        );
        for absent in ["RoleArn", "AssociatedAt"] {
            assert_eq!(
                member(&got, absent),
                Value::Null,
                "case {case:?} after disassociating: expected no {absent}, got {got}"
            );
        }

        // Asking again changes nothing and still succeeds: the association is gone, so
        // this call disassociated nothing.
        let (status, again) = disassociate(&router).await;
        assert_eq!(
            (status, member(&again, "DisassociatedAt")),
            (200, Value::Null),
            "case {case:?} disassociated twice: expected a success that disassociated \
             nothing, got {status} {again}"
        );
    }
}
