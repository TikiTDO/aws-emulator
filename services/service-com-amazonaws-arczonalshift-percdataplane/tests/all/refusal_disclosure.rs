// SPDX-License-Identifier: CC0-1.0

//! What a refusal of this service states about the request it refused, driven end to end
//! over the same `MultiServiceRouter` the emulator boots.
//!
//! Two rules, each asserted over a SET of inputs that differ in the VALUE the request
//! carried — the axis each rule generalises over — rather than over one recorded request.
//! The table over MEMBERS and constraint classes lives in `refusal_message_rules.rs`, and
//! the table over refused TARGETS in `access_denied_precedence.rs`; here the member is
//! held fixed and the value varies:
//!
//! 1. a `ValidationException` clause states the value the request carried ONLY for a
//!    member reached through another shape — an element of a collection, whose value is
//!    what names the one element among several that was refused. A member of the
//!    operation's own input shape, whether ARC read it from a URI label, a query
//!    parameter or the top level of the body, is stated by its POSITION ALONE and the
//!    refusal carries nothing of what the caller sent;
//! 2. an `AccessDeniedException` states a reason only where the refused call named a
//!    RESOURCE. A call whose target is a `zonalShiftId` ARC holds no shift for is denied
//!    with no reason at all — `AccessDeniedException$message` is optional
//!    (`smithy_query(query="shape", shape="AccessDeniedException")`) and a denial taken on
//!    a shift id, which is not a resource an IAM policy grants an action on, resolved no
//!    target to report about.
//!
//! Every constraint and value set quoted below is the model's (`smithy_query(query="shape",
//! shape="ZonalShiftId,ZonalShiftStatus,ControlConditionType,MetricIdentifier")`).

use super::zonal_shift_lifecycle::{call, emulator, encoded, error_code, seeded, start_body};
use service_com_amazonaws_arczonalshift_percdataplane::state::TARGET_UNAVAILABLE_MESSAGE;

/// `ZonalShiftId`'s `@pattern` as a refusal reports it: `^[A-Za-z0-9-]+$` without the
/// anchors that make the match total.
const ZONAL_SHIFT_ID_PATTERN: &str = "[A-Za-z0-9-]+";

/// Values outside `ZonalShiftId`'s `@pattern` but INSIDE its `@length` 6..36, so each one
/// breaks exactly one constraint and the envelope stays singular. They differ in the
/// character the pattern rejects and in their length, which is what the rule generalises
/// over.
const PATTERN_VIOLATING_IDS: &[&str] = &[
    "bad_id_1",
    "UNDER_SCORE_ID",
    "dots.and.dots",
    "tilde~shift",
    "comma,shift",
    "star*shift",
    "a_very_long_identifier_with_breaks_",
];

/// Values outside `ZonalShiftStatus`'s value set, each at least four characters so that
/// looking for it in a refusal body is a meaningful question. They differ in case, length
/// and how near they sit to a legal value.
const STATUSES_OUTSIDE_THE_SET: &[&str] = &["PENDING", "paused", "Cancelled", "ACTIVE-SHIFT"];

/// The admissible set a refusal states for a `status` filtering zonal shifts: the
/// published `ZonalShiftStatus` values in the order ARC names them.
const ZONAL_SHIFT_STATUS_SET: &str =
    "Member must satisfy enum value set: [EXPIRED, ACTIVE, CANCELED]";

/// The set a refusal states for a control condition's `type`: the published
/// `ControlConditionType` value plus the alarm source ARC names that the model does not
/// publish.
const CONTROL_CONDITION_TYPE_SET: &str =
    "Member must satisfy enum value set: [CARNAVAL, CLOUDWATCH]";

/// An alarm ARN inside `MetricIdentifier`'s `@length` 8..1024, so a row about a control
/// condition varies only the `type` it is about.
const ALARM_ARN: &str = "arn:aws:cloudwatch:us-east-1:111111111111:alarm:outcome";

/// An ARN-shaped `resourceIdentifier` ARC holds no managed resource for.
const UNREGISTERED_ARN: &str = "arn:aws:eks:us-east-1:111111111111:cluster/absent";

/// Well-formed `zonalShiftId`s — inside `@length` 6..36 and `@pattern` — that name no
/// stored shift, spanning each end of the length range and every kind of character the
/// pattern admits.
const UNKNOWN_IDS: &[&str] = &[
    "abcdef",
    "zs-0000009999",
    "Zs-UNKNOWN-0001",
    "123456",
    "A1b2C3d4E5f6A1b2C3d4E5f6A1b2C3d4E5f6",
];

/// The message of a refusal, with its status and code checked first.
async fn refused(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    method: &str,
    uri: &str,
    body_text: &str,
) -> String {
    let (status, body) = call(router, method, uri, body_text).await;
    assert_eq!(
        status, 400,
        "case {case:?}: a constraint violation is refused 400, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {body}"
    );
    let rendered = body.to_string();
    body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the refusal states a message, got {rendered}"))
        .to_string()
}

/// A practice-run configuration body whose outcome alarms carry the control condition
/// types given, so a row varies exactly the `type` it is about.
fn practice_run_body(types: &[&str]) -> String {
    serde_json::json!({
        "resourceIdentifier": UNREGISTERED_ARN,
        "outcomeAlarms": types
            .iter()
            .map(|alarm_type| {
                serde_json::json!({ "type": alarm_type, "alarmIdentifier": ALARM_ARN })
            })
            .collect::<Vec<_>>(),
    })
    .to_string()
}

#[tokio::test]
async fn a_member_of_the_input_shape_is_refused_by_position_alone() {
    let (router, _engine) = emulator();

    // A URI label, over both operations that take a shift id: the value the request put
    // in the path is not in the refusal.
    for id in PATTERN_VIOLATING_IDS {
        let expected = format!(
            "1 validation error detected: Value at 'zonalShiftId' failed to satisfy \
             constraint: Member must satisfy regular expression pattern: \
             {ZONAL_SHIFT_ID_PATTERN}"
        );
        for (method, payload) in [("DELETE", ""), ("PATCH", "{}")] {
            let case = format!("{method} /zonalshifts/{id}");
            let uri = format!("/zonalshifts/{}", encoded(id));
            let message = refused(&router, &case, method, &uri, payload).await;
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
            assert!(
                !message.contains(id),
                "case {case:?}: the refusal must not echo the submitted value {id:?}, \
                 got {message:?}"
            );
        }
    }

    // A query parameter: the same, with the value read off the request line.
    for status in STATUSES_OUTSIDE_THE_SET {
        let case = format!("GET /zonalshifts?status={status}");
        let expected = format!(
            "1 validation error detected: Value at 'status' failed to satisfy constraint: \
             {ZONAL_SHIFT_STATUS_SET}"
        );
        let uri = format!("/zonalshifts?status={}", encoded(status));
        let message = refused(&router, &case, "GET", &uri, "").await;
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        assert!(
            !message.contains(status),
            "case {case:?}: the refusal must not echo the submitted value {status:?}, \
             got {message:?}"
        );
    }

    // A top-level BODY member: `expiresIn` against its `@pattern`, with the lengths the
    // model admits (2..5) varied across the rows.
    for expires_in in ["7d", "10d", "1x", "999d"] {
        let case = format!("POST /zonalshifts expiresIn={expires_in}");
        let expected = "1 validation error detected: Value at 'expiresIn' failed to satisfy \
                        constraint: Member must satisfy regular expression pattern: \
                        ([1-9][0-9]*)(m|h)";
        let body_text = start_body(UNREGISTERED_ARN, "use1-az1", expires_in, "a shift");
        let message = refused(&router, &case, "POST", "/zonalshifts", &body_text).await;
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        assert!(
            !message.contains(expires_in),
            "case {case:?}: the refusal must not echo the submitted value \
             {expires_in:?}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_member_reached_through_a_collection_states_the_element_value() {
    // The other side of the rule: the element's value is what names WHICH element of the
    // list was refused, so it is stated — at whatever index the element sits.
    let (router, _engine) = emulator();
    let rows: [(&str, Vec<&str>, usize, &str); 4] = [
        ("one element", vec!["BOGUS_TYPE"], 1, "BOGUS_TYPE"),
        (
            "the second of two",
            vec!["CLOUDWATCH", "SOMETHING_ELSE"],
            2,
            "SOMETHING_ELSE",
        ),
        (
            "the third of three",
            vec!["CLOUDWATCH", "CLOUDWATCH", "metric"],
            3,
            "metric",
        ),
        ("a lower-cased legal value", vec!["cloudwatch"], 1, "cloudwatch"),
    ];
    for (case, types, index, refused_value) in rows {
        let expected = format!(
            "1 validation error detected: Value '{refused_value}' at \
             'outcomeAlarms.{index}.member.type' failed to satisfy constraint: \
             {CONTROL_CONDITION_TYPE_SET}"
        );
        let body_text = practice_run_body(&types);
        let message = refused(&router, case, "POST", "/configuration", &body_text).await;
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_denial_on_a_shift_id_states_no_reason_at_all() {
    let (router, _engine) = emulator();
    for id in UNKNOWN_IDS {
        for (method, payload) in [("DELETE", ""), ("PATCH", r#"{"comment":"a comment"}"#)] {
            let case = format!("{method} /zonalshifts/{id}");
            let (status, body) = call(&router, method, &format!("/zonalshifts/{id}"), payload).await;
            assert_eq!(
                status, 403,
                "case {case:?}: an unheld shift id is denied 403, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "AccessDeniedException",
                "case {case:?}: expected AccessDeniedException, got {body}"
            );
            assert!(
                body["message"].is_null() || body["message"].as_str() == Some(""),
                "case {case:?}: the denial states no reason, got {body}"
            );
            assert!(
                !body.to_string().contains(*id),
                "case {case:?}: the denial must not disclose the shift id, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_denial_on_a_resource_states_arcs_reason_about_it() {
    // The boundary of the second rule: the same 403, for a call that named a RESOURCE,
    // carries the reason ARC states about a resource it does not hold for the caller.
    let (router, _engine) = emulator();
    for (resource, class) in [
        (UNREGISTERED_ARN, "an ARN ARC has no record of"),
        (
            "arn:aws:elasticloadbalancing:us-east-1:111111111111:loadbalancer/app/absent/73e2d6bc24d8a067",
            "a load balancer ARN in another account",
        ),
    ] {
        let (status, body) = call(
            &router,
            "POST",
            "/zonalshifts",
            &start_body(resource, "use1-az1", "1h", "a shift"),
        )
        .await;
        assert_eq!(
            status, 403,
            "case {class:?}: an unheld resource is denied 403, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "AccessDeniedException",
            "case {class:?}: expected AccessDeniedException, got {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(TARGET_UNAVAILABLE_MESSAGE),
            "case {class:?}: expected the reason ARC states about a resource, got {body}"
        );
    }
}

#[tokio::test]
async fn a_shift_the_store_holds_is_acted_on_rather_than_denied() {
    // Both rules are computed from state, not answered unconditionally: a shift id the
    // store DOES hold is authorized, and the mutation reads back through the shift's own
    // reader.
    let (router, engine) = emulator();
    let (resource, zones) = seeded(&engine)[0].clone();
    let (status, started) = call(
        &router,
        "POST",
        "/zonalshifts",
        &start_body(&resource, &zones[0], "1h", "a held shift"),
    )
    .await;
    assert_eq!(status, 201, "the shift starts, got {status} {started}");
    let id = started["zonalShiftId"]
        .as_str()
        .expect("zonalShiftId is required")
        .to_string();

    let (status, updated) = call(
        &router,
        "PATCH",
        &format!("/zonalshifts/{id}"),
        r#"{"comment":"still held"}"#,
    )
    .await;
    assert_eq!(
        status, 200,
        "updating a held shift answers 200, got {status} {updated}"
    );
    assert_eq!(
        updated["comment"], "still held",
        "the comment the update carried is persisted, got {updated}"
    );

    let (status, view) = call(
        &router,
        "GET",
        &format!("/managedresources/{}", encoded(&resource)),
        "",
    )
    .await;
    assert_eq!(
        status, 200,
        "GetManagedResource answers 200, got {status} {view}"
    );
    let shifts = view["zonalShifts"]
        .as_array()
        .unwrap_or_else(|| panic!("zonalShifts is required, got {view}"));
    assert!(
        shifts.iter().any(|shift| {
            shift["zonalShiftId"].as_str() == Some(id.as_str())
                && shift["comment"].as_str() == Some("still held")
        }),
        "the updated shift {id} is reported for the resource, got {view}"
    );
}
