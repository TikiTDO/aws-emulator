// SPDX-License-Identifier: CC0-1.0

//! How ARC words a request refused for violating a modelled constraint.
//!
//! The rules under test, each asserted over a TABLE spanning the constraint classes
//! this service's model declares (`@pattern`, an enum's value set, `@length` at both
//! ends, `@range` at both ends, an absent `@required` member) rather than over one
//! recorded request:
//!
//! * the envelope, the clause and the requirement are the framework's — `N validation
//!   error(s) detected: <subject> at '<member path>' failed to satisfy constraint:
//!   <requirement>`;
//! * the SUBJECT follows the refused member's POSITION in the input: a member of the
//!   operation's own input shape — a URI label, a query parameter or a top-level body
//!   member — is stated by position alone (`Value at '<member>'`), and a member reached
//!   through another shape (an element of a collection) states the value that element
//!   held (`Value '<submitted>' at '<path>'`); a member the request OMITTED has no value
//!   at all and states `Value null at '<member>'`;
//! * a `@pattern` is reported as the expression a value must satisfy, WITHOUT the
//!   `^`/`$` anchors the model writes to make the match total;
//! * an enum's admissible set is the set the service names — the model's values in the
//!   model's declaration order for all but two of this service's enums: the zonal-shift
//!   statuses, which ARC names expired-first, and the practice-run control-condition
//!   type, whose set carries an alarm source the public model does not publish;
//! * the status is 400 and the code is `ValidationException` (`@httpError 400`) in every
//!   case, and a value INSIDE a declared bound is not refused at all.
//!
//! Every expression and value set below is quoted from the model
//! (`smithy_query(query="shape", shape="ZonalShiftId,ExpiresIn,ZonalShiftStatus,
//! AutoshiftExecutionStatus,ZonalAutoshiftStatus,AutoshiftObserverNotificationStatus,
//! ControlConditionType,MaxResults,ResourceIdentifier,AvailabilityZone,
//! ZonalShiftComment")`), and the rows cover members the issue that prompted this suite
//! did not cite — `expiresIn`, `comment`, `awayFrom`, `maxResults`,
//! `resourceIdentifier`, the enums other than `ZonalShiftStatus`, and a `type` nested
//! inside a list of structures — including members of operations that are still
//! scaffolded, because a declared constraint is a property of the INPUT SHAPE and is
//! refused before any handler runs.

use super::zonal_shift_lifecycle::{call, emulator, encoded, error_code};

/// `ZonalShiftId`'s `@pattern` (`^[A-Za-z0-9-]+$`) as a refusal reports it: the
/// expression, without the anchors that make the match total.
const ZONAL_SHIFT_ID_PATTERN: &str = "[A-Za-z0-9-]+";

/// `ExpiresIn`'s `@pattern` (`^([1-9][0-9]*)(m|h)$`), likewise unanchored.
const EXPIRES_IN_PATTERN: &str = "([1-9][0-9]*)(m|h)";

/// An ARN-shaped `resourceIdentifier` ARC holds no record of. Validation runs before
/// any lookup, so a row only needs an identifier the model accepts.
const UNREGISTERED_ARN: &str = "arn:aws:eks:us-east-1:111111111111:cluster/absent";

/// An alarm ARN inside `MetricIdentifier`'s `@length` 8..1024 and `@pattern` `^.*$`, so a
/// row about a control condition's `type` varies only that member.
const ALARM_ARN: &str = "arn:aws:cloudwatch:us-east-1:111111111111:alarm:outcome";

/// The admissible set a refusal states for a `status` filtering zonal shifts: the
/// published `ZonalShiftStatus` values, in the order ARC names them rather than the
/// order the model declares them in.
const ZONAL_SHIFT_STATUSES: &str =
    "Member must satisfy enum value set: [EXPIRED, ACTIVE, CANCELED]";

/// The admissible set a refusal states for a control condition's `type`: the published
/// `ControlConditionType` value plus the alarm source ARC names that the public model
/// does not publish.
const CONTROL_CONDITION_TYPES: &str =
    "Member must satisfy enum value set: [CARNAVAL, CLOUDWATCH]";

/// One refused request and the clause ARC states for it.
struct Row {
    /// What this row is a case of, named in every assertion.
    case: String,
    method: &'static str,
    uri: String,
    body: String,
    /// The dotted member path the clause names.
    member: &'static str,
    /// The requirement half, as the framework words it for this constraint class.
    requirement: String,
    /// What the request submitted: the value the clause must quote back.
    submitted: String,
}

impl Row {
    fn new(
        case: &str,
        method: &'static str,
        uri: &str,
        body: &str,
        member: &'static str,
        requirement: &str,
        submitted: &str,
    ) -> Self {
        Self {
            case: case.to_string(),
            method,
            uri: uri.to_string(),
            body: body.to_string(),
            member,
            requirement: requirement.to_string(),
            submitted: submitted.to_string(),
        }
    }

    /// The whole message this row must be answered with: the subject the member's
    /// position earns, then the position and the requirement.
    fn expected(&self) -> String {
        format!(
            "1 validation error detected: {} at '{}' failed to satisfy constraint: {}",
            self.subject(),
            self.member,
            self.requirement
        )
    }

    /// The clause's subject: the value this row submitted, quoted, for a member reached
    /// through another shape (a dotted path), and `Value` alone for a member of the
    /// operation's own input shape.
    fn subject(&self) -> String {
        if self.member.contains('.') {
            format!("Value '{}'", self.submitted)
        } else {
            "Value".to_string()
        }
    }
}

/// A `StartZonalShift` body with one member replaced, so a row varies exactly the
/// member it is about.
fn start_body(away_from: &str, expires_in: &str, comment: &str) -> String {
    serde_json::json!({
        "resourceIdentifier": UNREGISTERED_ARN,
        "awayFrom": away_from,
        "expiresIn": expires_in,
        "comment": comment,
    })
    .to_string()
}

/// A practice-run configuration body whose outcome alarms are the control conditions
/// given, so a row varies exactly the `type` it is about. The list satisfies
/// `OutcomeAlarms`'s `@length` 1..10.
fn practice_run_body(outcome_alarm_types: &[&str]) -> String {
    serde_json::json!({
        "resourceIdentifier": UNREGISTERED_ARN,
        "outcomeAlarms": outcome_alarm_types
            .iter()
            .map(|alarm_type| {
                serde_json::json!({ "type": alarm_type, "alarmIdentifier": ALARM_ARN })
            })
            .collect::<Vec<_>>(),
    })
    .to_string()
}

/// One row per constraint class per member: the table the rule is asserted over.
fn rows() -> Vec<Row> {
    let too_long_comment = "c".repeat(129);
    let too_long_away_from = "a".repeat(21);
    let too_long_id = "z".repeat(37);
    vec![
        // `@pattern`, on a URI label, for both operations that take a shift id.
        Row::new(
            "CancelZonalShift/zonalShiftId outside its pattern",
            "DELETE",
            "/zonalshifts/bad_id_1",
            "",
            "zonalShiftId",
            &format!("Member must satisfy regular expression pattern: {ZONAL_SHIFT_ID_PATTERN}"),
            "bad_id_1",
        ),
        Row::new(
            "UpdateZonalShift/zonalShiftId outside its pattern",
            "PATCH",
            "/zonalshifts/bad.id.1",
            "{}",
            "zonalShiftId",
            &format!("Member must satisfy regular expression pattern: {ZONAL_SHIFT_ID_PATTERN}"),
            "bad.id.1",
        ),
        // `@pattern`, on a BODY member of another shape.
        Row::new(
            "StartZonalShift/expiresIn outside its pattern",
            "POST",
            "/zonalshifts",
            &start_body("use1-az1", "7d", "a shift"),
            "expiresIn",
            &format!("Member must satisfy regular expression pattern: {EXPIRES_IN_PATTERN}"),
            "7d",
        ),
        // `@length`, each end of the range, on three different members.
        Row::new(
            "CancelZonalShift/zonalShiftId below its minimum length",
            "DELETE",
            "/zonalshifts/short",
            "",
            "zonalShiftId",
            "Member must have length greater than or equal to 6",
            "short",
        ),
        Row::new(
            "CancelZonalShift/zonalShiftId above its maximum length",
            "DELETE",
            &format!("/zonalshifts/{too_long_id}"),
            "",
            "zonalShiftId",
            "Member must have length less than or equal to 36",
            &too_long_id,
        ),
        Row::new(
            "ListZonalShifts/resourceIdentifier below its minimum length",
            "GET",
            "/zonalshifts?resourceIdentifier=short",
            "",
            "resourceIdentifier",
            "Member must have length greater than or equal to 8",
            "short",
        ),
        Row::new(
            "StartZonalShift/comment above its maximum length",
            "POST",
            "/zonalshifts",
            &start_body("use1-az1", "1h", &too_long_comment),
            "comment",
            "Member must have length less than or equal to 128",
            &too_long_comment,
        ),
        Row::new(
            "StartZonalShift/awayFrom above its maximum length",
            "POST",
            "/zonalshifts",
            &start_body(&too_long_away_from, "1h", "a shift"),
            "awayFrom",
            "Member must have length less than or equal to 20",
            &too_long_away_from,
        ),
        // An enum's value set, on a query member, a body member and a member nested in a
        // list of structures — including the enums the issue that prompted this suite did
        // not cite, and both shapes whose set ARC names for itself.
        Row::new(
            "ListZonalShifts/status outside ZonalShiftStatus",
            "GET",
            "/zonalshifts?status=PENDING",
            "",
            "status",
            ZONAL_SHIFT_STATUSES,
            "PENDING",
        ),
        Row::new(
            "ListAutoshifts/status outside AutoshiftExecutionStatus",
            "GET",
            "/autoshifts?status=PENDING",
            "",
            "status",
            "Member must satisfy enum value set: [ACTIVE, COMPLETED]",
            "PENDING",
        ),
        Row::new(
            "UpdateZonalAutoshiftConfiguration/zonalAutoshiftStatus outside its value set",
            "PUT",
            &format!("/managedresources/{}", encoded(UNREGISTERED_ARN)),
            r#"{"zonalAutoshiftStatus":"PAUSED"}"#,
            "zonalAutoshiftStatus",
            "Member must satisfy enum value set: [ENABLED, DISABLED]",
            "PAUSED",
        ),
        Row::new(
            "UpdateAutoshiftObserverNotificationStatus/status outside its value set",
            "PUT",
            "/autoshift-observer-notification",
            r#"{"status":"BOGUS"}"#,
            "status",
            "Member must satisfy enum value set: [ENABLED, DISABLED]",
            "BOGUS",
        ),
        // An enum nested in a list of structures: the clause names the element's
        // position, and the set stated is the one the service names for this member.
        Row::new(
            "CreatePracticeRunConfiguration/outcomeAlarms type outside its value set",
            "POST",
            "/configuration",
            &practice_run_body(&["BOGUS_TYPE"]),
            "outcomeAlarms.1.member.type",
            CONTROL_CONDITION_TYPES,
            "BOGUS_TYPE",
        ),
        Row::new(
            "CreatePracticeRunConfiguration/outcomeAlarms type, second element",
            "POST",
            "/configuration",
            &practice_run_body(&["CLOUDWATCH", "SOMETHING_ELSE"]),
            "outcomeAlarms.2.member.type",
            CONTROL_CONDITION_TYPES,
            "SOMETHING_ELSE",
        ),
        Row::new(
            "UpdatePracticeRunConfiguration/blockingAlarms type outside its value set",
            "PATCH",
            &format!("/configuration/{}", encoded(UNREGISTERED_ARN)),
            &serde_json::json!({
                "blockingAlarms": [{ "type": "NOT_AN_ALARM", "alarmIdentifier": ALARM_ARN }],
            })
            .to_string(),
            "blockingAlarms.1.member.type",
            CONTROL_CONDITION_TYPES,
            "NOT_AN_ALARM",
        ),
        // `@range`, each end of the bound the model publishes, on both list operations
        // that page.
        Row::new(
            "ListZonalShifts/maxResults below its minimum",
            "GET",
            "/zonalshifts?maxResults=0",
            "",
            "maxResults",
            "Member must have value greater than or equal to 1",
            "0",
        ),
        Row::new(
            "ListZonalShifts/maxResults above its maximum",
            "GET",
            "/zonalshifts?maxResults=101",
            "",
            "maxResults",
            "Member must have value less than or equal to 100",
            "101",
        ),
        Row::new(
            "ListManagedResources/maxResults above its maximum",
            "GET",
            "/managedresources?maxResults=1000",
            "",
            "maxResults",
            "Member must have value less than or equal to 100",
            "1000",
        ),
        Row::new(
            "ListAutoshifts/maxResults above its maximum",
            "GET",
            "/autoshifts?maxResults=101",
            "",
            "maxResults",
            "Member must have value less than or equal to 100",
            "101",
        ),
    ]
}

/// The message of a refusal, with the status and code checked first.
async fn refusal_message(row: &Row) -> String {
    let (router, _engine) = emulator();
    let (status, body) = call(&router, row.method, &row.uri, &row.body).await;
    assert_eq!(
        status, 400,
        "case {:?}: a constraint violation is refused 400, got {status} {body}",
        row.case
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case {:?}: expected ValidationException, got {body}",
        row.case
    );
    body["message"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: the refusal states a message, got {body}",
                row.case
            )
        })
        .to_string()
}

#[tokio::test]
async fn a_clause_states_the_subject_the_refused_members_position_earns() {
    for row in rows() {
        let message = refusal_message(&row).await;
        assert_eq!(
            message,
            row.expected(),
            "case {:?}: expected {:?}, got {message:?}",
            row.case,
            row.expected()
        );
        if row.member.contains('.') {
            assert!(
                message.contains(&format!("Value '{}' at '{}'", row.submitted, row.member)),
                "case {:?}: a member reached through a collection states the element's \
                 value, got {message:?}",
                row.case
            );
        } else {
            assert!(
                message.starts_with(&format!("1 validation error detected: Value at '{}'", row.member)),
                "case {:?}: a member of the input shape is stated by position alone, \
                 got {message:?}",
                row.case
            );
            assert!(
                !message.contains(&format!("'{}'", row.submitted)),
                "case {:?}: the clause must not echo the submitted value {:?}, got {message:?}",
                row.case,
                row.submitted
            );
        }
    }
}

#[tokio::test]
async fn a_pattern_is_reported_without_the_anchors_the_model_spells_it_with() {
    for row in rows() {
        if !row.requirement.contains("regular expression pattern") {
            continue;
        }
        let message = refusal_message(&row).await;
        for anchored in [
            format!("^{ZONAL_SHIFT_ID_PATTERN}$"),
            format!("^{EXPIRES_IN_PATTERN}$"),
        ] {
            assert!(
                !message.contains(&anchored),
                "case {:?}: the clause must not state the anchored pattern {anchored:?}, \
                 got {message:?}",
                row.case
            );
        }
        assert!(
            message.ends_with(&row.requirement),
            "case {:?}: the clause ends with the unanchored expression {:?}, got {message:?}",
            row.case,
            row.requirement
        );
    }
}

#[tokio::test]
async fn an_absent_required_member_keeps_the_clause_that_states_an_absence() {
    // The boundary of the subject rule: an absent member HELD no value, which is neither
    // a value to state nor a position stated alone, so its clause stays the framework's
    // `Value null at '<member>' …`.
    let (router, _engine) = emulator();
    let (status, body) = call(&router, "POST", "/zonalshifts", "{}").await;
    assert_eq!(
        status, 400,
        "an absent required member is refused 400, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "expected ValidationException, got {body}"
    );
    let message = body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("the refusal states a message, got {body}"));
    assert!(
        message.starts_with("1 validation error detected: Value null at '"),
        "an absent member is stated as \"Value null at '<member>'\", got {message:?}"
    );
    assert!(
        message.ends_with("failed to satisfy constraint: Member must not be null"),
        "an absent member fails the not-null requirement, got {message:?}"
    );
}

#[tokio::test]
async fn several_violations_are_counted_and_joined_in_one_envelope() {
    // Two members outside their constraints in one request: the count is the framework's
    // plural envelope, and EVERY clause states its own subject — not just the first.
    let (router, _engine) = emulator();
    let (status, body) = call(
        &router,
        "GET",
        "/zonalshifts?status=PENDING&maxResults=0",
        "",
    )
    .await;
    assert_eq!(
        status, 400,
        "two constraint violations are refused 400, got {status} {body}"
    );
    let message = body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("the refusal states a message, got {body}"));
    assert!(
        message.starts_with("2 validation errors detected: "),
        "the envelope counts both violations, got {message:?}"
    );
    for clause in [
        "Value at 'status' failed to satisfy constraint: Member must satisfy enum \
         value set: [EXPIRED, ACTIVE, CANCELED]",
        "Value at 'maxResults' failed to satisfy constraint: Member must have value \
         greater than or equal to 1",
    ] {
        assert!(
            message.contains(clause),
            "the envelope states {clause:?}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_value_inside_a_declared_bound_is_not_refused() {
    // The other side of every row above: each published bound, from the inside, is
    // accepted — so the refusals are computed from the constraint and not from the shape
    // of the request. What the operation then answers (200, a denial, or the 501 of an
    // operation that is still scaffolded) is not this suite's business; that it is not a
    // `ValidationException` is.
    let (router, _engine) = emulator();
    let accepted: Vec<(&str, &str, String, String)> = vec![
        (
            "maxResults at its minimum",
            "GET",
            "/zonalshifts?maxResults=1".to_string(),
            String::new(),
        ),
        (
            "maxResults at its maximum",
            "GET",
            "/zonalshifts?maxResults=100".to_string(),
            String::new(),
        ),
        (
            "status inside ZonalShiftStatus",
            "GET",
            "/zonalshifts?status=CANCELED".to_string(),
            String::new(),
        ),
        (
            "status inside AutoshiftExecutionStatus",
            "GET",
            "/autoshifts?status=COMPLETED".to_string(),
            String::new(),
        ),
        (
            "resourceIdentifier at its minimum length",
            "GET",
            "/zonalshifts?resourceIdentifier=abcdefgh".to_string(),
            String::new(),
        ),
        (
            "zonalShiftId at its minimum length",
            "DELETE",
            format!("/zonalshifts/{}", "a".repeat(6)),
            String::new(),
        ),
        (
            "zonalShiftId at its maximum length",
            "DELETE",
            format!("/zonalshifts/{}", "a".repeat(36)),
            String::new(),
        ),
        (
            // "1440m" is five characters, the most `ExpiresIn` admits, and inside its
            // pattern.
            "expiresIn at its maximum length",
            "POST",
            "/zonalshifts".to_string(),
            start_body("use1-az1", "1440m", "a shift"),
        ),
        (
            "zonalAutoshiftStatus inside its value set",
            "PUT",
            format!("/managedresources/{}", encoded(UNREGISTERED_ARN)),
            r#"{"zonalAutoshiftStatus":"ENABLED"}"#.to_string(),
        ),
        (
            "status inside AutoshiftObserverNotificationStatus",
            "PUT",
            "/autoshift-observer-notification".to_string(),
            r#"{"status":"DISABLED"}"#.to_string(),
        ),
        (
            "a control condition type inside the published value set",
            "POST",
            "/configuration".to_string(),
            practice_run_body(&["CLOUDWATCH"]),
        ),
    ];
    for (case, method, uri, body_text) in accepted {
        let (status, body) = call(&router, method, &uri, &body_text).await;
        assert_ne!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: a value inside its bound is not a validation error, \
             got {status} {body}"
        );
    }
}
