// SPDX-License-Identifier: CC0-1.0

//! ARN-shaped inputs are validated before any resource is looked up.
//!
//! The rule these suites pin: an input member targeting an `@arnReference` shape the
//! model gives no `@pattern` (`QuantumTaskArn`, `DeviceArn` — the boundary validator
//! enforces the `@pattern` the other ARN shapes carry) must be checked for Braket ARN
//! syntax by the handler; a value that is not an ARN of this service and resource type
//! is invalid INPUT (HTTP 400 `ValidationException`), and only a well-formed ARN naming
//! no stored resource is a missing resource (HTTP 404 `ResourceNotFoundException`).
//!
//! Each suite asserts that over a TABLE spanning the class of malformed values — the
//! ARN prefix, the field count, the partition, the service namespace, the region, the
//! account id, the resource type and the resource id — and over a table of well-formed
//! ARNs that differ in partition, region, account and id form. The recorded request
//! (`not-a-valid-quantum-task-arn`) is one row of the first table.
//!
//! The one exception is the last suite, on a member the model DOES give a `@pattern`
//! and that travels in the request BODY: that refusal belongs to Braket's front door,
//! which answers a body outside the modelled input with 400 `BadRequestException` (see
//! `request_body_rejection.rs`), so it is asserted as such here.
//!
//! Everything goes over HTTP through the production router, reusing the harness in
//! `quantum_task_loop.rs`.

use serde_json::{json, Value};

use crate::quantum_task_loop::{create_body, Braket, Reply};

/// Values that are not Braket quantum-task ARNs, one row per part of the ARN grammar
/// a value can miss. `(case, value)`.
const MALFORMED_QUANTUM_TASK_ARNS: &[(&str, &str)] = &[
    // The recorded request this rule was filed for.
    ("not an ARN at all", "not-a-valid-quantum-task-arn"),
    ("a bare resource id", "00000000-0000-4000-8000-000000000000"),
    ("the arn prefix in upper case", "ARN:aws:braket:us-east-1:000000000000:quantum-task/t1"),
    ("too few fields", "arn:aws:braket:us-east-1:000000000000"),
    (
        "a partition outside the aws family",
        "arn:azure:braket:us-east-1:000000000000:quantum-task/t1",
    ),
    ("another service's namespace", "arn:aws:s3:us-east-1:000000000000:quantum-task/t1"),
    (
        "another Braket resource type",
        "arn:aws:braket:us-east-1:000000000000:job/j1",
    ),
    (
        "a resource type this one is only a prefix of",
        "arn:aws:braket:us-east-1:000000000000:quantum-tasks/t1",
    ),
    ("no resource id", "arn:aws:braket:us-east-1:000000000000:quantum-task"),
    ("an empty resource id", "arn:aws:braket:us-east-1:000000000000:quantum-task/"),
    (
        "an account id shorter than 12 digits",
        "arn:aws:braket:us-east-1:12345:quantum-task/t1",
    ),
    (
        "an account id that is not all digits",
        "arn:aws:braket:us-east-1:00000000000a:quantum-task/t1",
    ),
    (
        "a region in upper case",
        "arn:aws:braket:US-EAST-1:000000000000:quantum-task/t1",
    ),
];

/// Well-formed quantum-task ARNs this emulator holds no record for: the shape of the
/// value varies (partition, region, account, id), the answer does not. `(case, value)`.
pub(crate) const ABSENT_QUANTUM_TASK_ARNS: &[(&str, &str)] = &[
    (
        "the form this emulator mints",
        "arn:aws:braket:eu-west-1:000000000000:quantum-task/00000000-0000-4000-8000-000000000000",
    ),
    (
        "the China partition",
        "arn:aws-cn:braket:cn-north-1:123456789012:quantum-task/t1",
    ),
    (
        "the GovCloud partition",
        "arn:aws-us-gov:braket:us-gov-west-1:123456789012:quantum-task/t1",
    ),
    // `BraketResourceArn`'s own pattern permits an empty region, and the device ARNs
    // Braket publishes carry neither region nor account: an empty field is not what
    // makes an ARN malformed.
    ("an empty region and account", "arn:aws:braket:::quantum-task/t1"),
    (
        "a resource id carrying slashes",
        "arn:aws:braket:us-east-1:000000000000:quantum-task/nested/id",
    ),
];

/// Values that are not Braket device ARNs. `(case, value)`.
const MALFORMED_DEVICE_ARNS: &[(&str, &str)] = &[
    ("not an ARN at all", "not-an-arn"),
    ("a bare device name", "sv1"),
    (
        "the resource path with no arn prefix",
        "braket:::device/quantum-simulator/amazon/sv1",
    ),
    ("another service's namespace", "arn:aws:s3:::device/quantum-simulator/amazon/sv1"),
    (
        "another Braket resource type",
        "arn:aws:braket:us-east-1:000000000000:quantum-task/t1",
    ),
    ("no resource id", "arn:aws:braket:::device"),
    ("an empty resource id", "arn:aws:braket:::device/"),
    (
        "an account id shorter than 12 digits",
        "arn:aws:braket:us-east-1:1:device/qpu/ionq/Aria-1",
    ),
    (
        "a partition outside the aws family",
        "arn:gcp:braket:::device/quantum-simulator/amazon/sv1",
    ),
];

/// Well-formed device ARNs the seeded inventory does not hold. `(case, value)`.
const ABSENT_DEVICE_ARNS: &[(&str, &str)] = &[
    (
        "a simulator that is not seeded",
        "arn:aws:braket:::device/quantum-simulator/amazon/nope",
    ),
    (
        "a QPU that is not seeded",
        "arn:aws:braket:us-east-1::device/qpu/nobody/Nothing",
    ),
    (
        "another partition",
        "arn:aws-cn:braket:cn-north-1:123456789012:device/qpu/nobody/Nothing",
    ),
];

/// Assert `reply` is the 400 `ValidationException` a malformed ARN earns, naming the
/// member that was wrong.
fn assert_invalid_arn(reply: &Reply, case: &str, member: &str) {
    assert_eq!(
        reply.status, 400,
        "case {case:?}: expected HTTP 400 for a value that is not an ARN, got {} {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error.as_deref(),
        Some("ValidationException"),
        "case {case:?}: expected ValidationException, got {:?} ({})",
        reply.error,
        reply.body
    );
    let message = reply
        .body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        message.contains(member),
        "case {case:?}: expected the message to name the member {member:?}, got {message:?}"
    );
}

/// Assert `reply` is the 404 `ResourceNotFoundException` a well-formed ARN naming
/// nothing earns.
fn assert_not_found(reply: &Reply, case: &str) {
    assert_eq!(
        reply.status, 404,
        "case {case:?}: expected HTTP 404 for a well-formed ARN naming no resource, got {} {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error.as_deref(),
        Some("ResourceNotFoundException"),
        "case {case:?}: expected ResourceNotFoundException, got {:?} ({})",
        reply.error,
        reply.body
    );
}

/// `GetQuantumTask`: a `quantumTaskArn` that is not a quantum-task ARN is refused as
/// invalid input; a well-formed one naming no record is a missing resource; and the ARN
/// the service itself mints is read back.
#[tokio::test]
async fn get_quantum_task_separates_a_malformed_arn_from_an_absent_task() {
    let braket = Braket::new();

    for (case, value) in MALFORMED_QUANTUM_TASK_ARNS {
        let reply = braket.get_task(value).await;
        assert_invalid_arn(&reply, case, "quantumTaskArn");
    }

    for (case, value) in ABSENT_QUANTUM_TASK_ARNS {
        let reply = braket.get_task(value).await;
        assert_not_found(&reply, case);
    }

    // The validation refuses no ARN this service mints: a created task still reads back.
    let device_arn = braket.an_online_device().await;
    let created = braket
        .create_task(create_body("arn-syntax-get", &device_arn, 1))
        .await;
    assert_eq!(
        created.status, 201,
        "case a created task: expected HTTP 201, got {} {}",
        created.status, created.body
    );
    let arn = created.str_member("quantumTaskArn");
    let read = braket.get_task(&arn).await;
    assert_eq!(
        read.status, 200,
        "case a created task {arn}: expected HTTP 200, got {} {}",
        read.status, read.body
    );
    assert_eq!(
        read.str_member("quantumTaskArn"),
        arn,
        "case a created task {arn}: expected the ARN to read back, got {}",
        read.body
    );
}

/// `CancelQuantumTask` is held to the SAME check on the SAME label: the two operations
/// cannot drift, because both call the one shared validator.
#[tokio::test]
async fn cancel_quantum_task_separates_a_malformed_arn_from_an_absent_task() {
    let braket = Braket::new();

    for (index, (case, value)) in MALFORMED_QUANTUM_TASK_ARNS.iter().enumerate() {
        let reply = braket
            .cancel_task(value, &format!("arn-syntax-cancel-bad-{index}"))
            .await;
        assert_invalid_arn(&reply, case, "quantumTaskArn");
    }

    for (index, (case, value)) in ABSENT_QUANTUM_TASK_ARNS.iter().enumerate() {
        let reply = braket
            .cancel_task(value, &format!("arn-syntax-cancel-absent-{index}"))
            .await;
        assert_not_found(&reply, case);
    }

    // And a task that exists is still cancelled.
    let device_arn = braket.an_online_device().await;
    let created = braket
        .create_task(create_body("arn-syntax-cancel", &device_arn, 1))
        .await;
    assert_eq!(
        created.status, 201,
        "case a created task: expected HTTP 201, got {} {}",
        created.status, created.body
    );
    let arn = created.str_member("quantumTaskArn");
    let cancelled = braket.cancel_task(&arn, "arn-syntax-cancel-ok").await;
    assert_eq!(
        cancelled.status, 200,
        "case cancel a created task {arn}: expected HTTP 200, got {} {}",
        cancelled.status, cancelled.body
    );
    assert_eq!(
        cancelled.str_member("quantumTaskArn"),
        arn,
        "case cancel a created task {arn}: expected the ARN back, got {}",
        cancelled.body
    );
}

/// `GetDevice` carries the other `@arnReference` shape with no `@pattern`
/// (`DeviceArn`), so the same rule holds for it: malformed is 400, well-formed and
/// absent is 404, and every seeded device still answers 200.
#[tokio::test]
async fn get_device_separates_a_malformed_arn_from_an_absent_device() {
    let braket = Braket::new();

    for (case, value) in MALFORMED_DEVICE_ARNS {
        let reply = braket.get_device(value).await;
        assert_invalid_arn(&reply, case, "deviceArn");
    }

    for (case, value) in ABSENT_DEVICE_ARNS {
        let reply = braket.get_device(value).await;
        assert_not_found(&reply, case);
    }

    // The published ARN forms the inventory itself carries are accepted.
    let device_arn = braket.an_online_device().await;
    let reply = braket.get_device(&device_arn).await;
    assert_eq!(
        reply.status, 200,
        "case seeded device {device_arn}: expected HTTP 200, got {} {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.str_member("deviceArn"),
        device_arn,
        "case seeded device {device_arn}: expected the ARN back, got {}",
        reply.body
    );
}

/// `CreateQuantumTask` takes the same `DeviceArn` shape in its BODY. It models no
/// `ResourceNotFoundException`, so a malformed ARN and an unknown device are both 400 —
/// but the malformed one is refused for its syntax, naming the member, before the
/// inventory is consulted.
#[tokio::test]
async fn create_quantum_task_refuses_a_device_arn_that_is_not_an_arn() {
    let braket = Braket::new();

    for (index, (case, value)) in MALFORMED_DEVICE_ARNS.iter().enumerate() {
        let reply = braket
            .create_task(create_body(
                &format!("arn-syntax-create-bad-{index}"),
                value,
                1,
            ))
            .await;
        assert_invalid_arn(&reply, case, "deviceArn");
    }

    // A well-formed device ARN that is simply not in the inventory is also a 400 here
    // (this operation models no 404), and no task is created either way: the ARN the
    // one legitimate create returns is the only task the store can answer for.
    for (index, (case, value)) in ABSENT_DEVICE_ARNS.iter().enumerate() {
        let reply = braket
            .create_task(create_body(
                &format!("arn-syntax-create-absent-{index}"),
                value,
                1,
            ))
            .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400 for a device the inventory does not hold, \
             got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?} ({})",
            reply.error,
            reply.body
        );
    }

    let device_arn = braket.an_online_device().await;
    let created = braket
        .create_task(create_body("arn-syntax-create-ok", &device_arn, 1))
        .await;
    assert_eq!(
        created.status, 201,
        "case a seeded device {device_arn}: expected HTTP 201, got {} {}",
        created.status, created.body
    );
}

/// The validator is reached through the wire for a member the model gives a
/// `@pattern`, too: `Association$arn` carries
/// `^arn:aws[a-z\-]*:braket:[a-z0-9\-]*:[0-9]{12}:.*$`, which the boundary enforces —
/// so a malformed association ARN is already a 400 and needs no handler check. This row
/// is what keeps that claim honest.
///
/// It is answered by Braket's FRONT DOOR rather than by the service's own
/// `ValidationException`, because the member travels in the request BODY: a body that
/// breaks a modelled constraint is rejected ahead of the service as 400
/// `BadRequestException` / "Invalid request body"
/// (`request_body_rejection.rs`, and the recorded `CreateSpendingLimit` / `CreateJob`
/// `@pattern` refusals it pins). What this suite's other rows refuse — a value that is
/// not an ARN on a shape the model gives NO `@pattern` — is the handler's own check and
/// stays the modelled `ValidationException` naming the member.
#[tokio::test]
async fn a_modelled_pattern_on_an_arn_member_is_enforced_at_the_boundary() {
    let braket = Braket::new();
    let device_arn = braket.an_online_device().await;

    let mut body = create_body("arn-syntax-association", &device_arn, 1);
    body["associations"] = json!([{
        "arn": "not-a-reservation-arn",
        "type": "RESERVATION_TIME_WINDOW_ARN",
    }]);
    let reply = braket.create_task(body).await;
    assert_eq!(
        reply.status, 400,
        "case a malformed association ARN: expected HTTP 400, got {} {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error.as_deref(),
        Some("BadRequestException"),
        "case a malformed association ARN: expected the front door's BadRequestException \
         for a body member outside its modelled @pattern, got {:?} ({})",
        reply.error,
        reply.body
    );
}
