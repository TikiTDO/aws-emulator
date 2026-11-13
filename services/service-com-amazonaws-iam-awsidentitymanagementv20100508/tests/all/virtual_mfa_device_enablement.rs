// SPDX-License-Identifier: CC0-1.0

//! Enabling one of the account's virtual MFA devices for an IAM user.
//!
//! The rule under test: **`EnableMFADevice` associates the device its `SerialNumber` names
//! with the named user, and only when the two authentication codes the request presents are
//! the pass codes that device emits in two consecutive time steps.** "Enables the specified
//! MFA device and associates it with the specified IAM user" with "an authentication code
//! emitted by the device" and "a subsequent authentication code emitted by the device"
//! (`EnableMFADevice`, `$AuthenticationCode1`, `$AuthenticationCode2`) — so the codes are
//! authenticated against the seed the create provisioned the device with, a pair that is not
//! two codes in a row proves nothing and is refused `InvalidAuthenticationCode` (HTTP 403)
//! with the device left unassigned, and a device another user already holds cannot be enabled
//! for a second one.
//!
//! What a successful enable records is read back through the account's own listing of its
//! devices (`ListVirtualMFADevices`), which is where a device's user and enable date are
//! reported.
//!
//! The suite computes the pass codes itself, from RFC 6238's definition over the seed the
//! create answered with — the arithmetic a holder of the device does — so a wrong algorithm in
//! the service cannot be the reason a row passes.
//!
//! The table's rows differ along what the rule generalises over: which pair of codes is
//! presented (the two in a row, the same code twice, the pair reversed, a pair from another
//! time step), whether the serial names a device the account holds, whether the account holds
//! the named user, and whether the device is already enabled for someone.
//!
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the other suites in
//! this crate share.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{call, element, elements, error_code, router};

/// The IAM user every row enables a device for.
const USER: &str = "mfa-operator";

/// How long one time step of a TOTP pass code lasts: RFC 6238's recommended default, which is
/// what an authenticator app provisioned from an `otpauth://totp/` URI naming no period uses.
const TIME_STEP: Duration = Duration::from_secs(30);

/// How many digits one pass code carries: "the format for this parameter is a string of six
/// digits" (`EnableMFADeviceRequest$AuthenticationCode1`).
const CODE_DIGITS: usize = 6;

/// Create the IAM user every row names.
async fn given_user(router: &MultiServiceRouter, name: &str) {
    let (status, body) = call(router, "CreateUser", &[("UserName", name)]).await;
    assert_eq!(status, 200, "the user {name:?} must be created, got {body}");
}

/// Provision a virtual MFA device, and answer its serial number and the base32 seed the
/// create reported — the two values a holder of the device needs.
async fn given_device(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let (status, body) = call(
        router,
        "CreateVirtualMFADevice",
        &[("VirtualMFADeviceName", name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "the device {name:?} must be provisioned, got {body}"
    );
    let serial_number = element(&body, "SerialNumber");
    // `Base32StringSeed` is a blob, so the wire carries it base64-encoded.
    let encoded = element(&body, "Base32StringSeed");
    let seed = data_encoding::BASE64
        .decode(encoded.as_bytes())
        .unwrap_or_else(|error| panic!("the seed must be base64, got {encoded:?}: {error}"));
    (
        serial_number,
        String::from_utf8(seed).expect("the seed is base32 text"),
    )
}

/// The pass code a device provisioned with `seed` emits in the time step `step`, computed here
/// rather than asked of the service (see the module docs): RFC 6238's TOTP over RFC 4226's
/// HOTP, HMAC-SHA-1 of the counter, dynamically truncated and taken to six digits.
fn pass_code(seed: &str, step: u64) -> String {
    use hmac::Mac;

    let key = data_encoding::BASE32_NOPAD
        .decode(seed.trim_end_matches('=').as_bytes())
        .unwrap_or_else(|error| panic!("the seed must be base32, got {seed:?}: {error}"));
    let mut mac =
        hmac::Hmac::<sha1::Sha1>::new_from_slice(&key).expect("HMAC takes a key of any length");
    mac.update(&step.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = usize::from(digest[digest.len() - 1] & 0x0f);
    let truncated = u32::from_be_bytes([
        digest[offset] & 0x7f,
        digest[offset + 1],
        digest[offset + 2],
        digest[offset + 3],
    ]);
    format!("{:0width$}", truncated % 1_000_000, width = CODE_DIGITS)
}

/// The time step the instant `now` falls in.
fn current_step(now: SystemTime) -> u64 {
    now.duration_since(UNIX_EPOCH)
        .expect("the clock is past the epoch")
        .as_secs()
        / TIME_STEP.as_secs()
}

/// WHICH pair of codes a row presents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Codes {
    /// The code of this time step followed by the next one's: what the device emits.
    InSequence,
    /// The same code twice, which is one code rather than two in a row.
    Repeated,
    /// The pair in the wrong order.
    Reversed,
    /// A pair from a time step far from this one, which the drift allowance cannot reach.
    FromAnotherTimeStep,
}

impl Codes {
    /// The pair as this row presents it, for a device provisioned with `seed`.
    fn pair(self, seed: &str) -> (String, String) {
        let step = current_step(SystemTime::now());
        let (first, second) = (pass_code(seed, step), pass_code(seed, step + 1));
        match self {
            Self::InSequence => (first, second),
            Self::Repeated => (first.clone(), first),
            Self::Reversed => (second, first),
            Self::FromAnotherTimeStep => (pass_code(seed, step + 500), pass_code(seed, step + 501)),
        }
    }
}

/// One row: what is presented, and what IAM answers.
struct Row {
    case: &'static str,
    codes: Codes,
    /// The expected HTTP status.
    status: u16,
    /// The expected error code, or `None` for a row that must be served.
    error: Option<&'static str>,
}

/// The code table: only the pair the device really emits enables it.
const ROWS: &[Row] = &[
    Row {
        case: "the two codes the device emits in a row",
        codes: Codes::InSequence,
        status: 200,
        error: None,
    },
    Row {
        case: "the same code twice",
        codes: Codes::Repeated,
        status: 403,
        error: Some("InvalidAuthenticationCode"),
    },
    Row {
        case: "the pair in the wrong order",
        codes: Codes::Reversed,
        status: 403,
        error: Some("InvalidAuthenticationCode"),
    },
    Row {
        case: "a pair from another time step",
        codes: Codes::FromAnotherTimeStep,
        status: 403,
        error: Some("InvalidAuthenticationCode"),
    },
];

/// RULE: a device is enabled only by the two pass codes it emits in a row; any other pair is
/// refused `InvalidAuthenticationCode` and leaves the device assigned to nobody.
#[tokio::test]
async fn only_two_consecutive_pass_codes_of_the_device_enable_it() {
    for row in ROWS {
        let router = router();
        given_user(&router, USER).await;
        let (serial_number, seed) = given_device(&router, "operator-device").await;
        let (first, second) = row.codes.pair(&seed);
        let (status, body) = call(
            &router,
            "EnableMFADevice",
            &[
                ("UserName", USER),
                ("SerialNumber", &serial_number),
                ("AuthenticationCode1", &first),
                ("AuthenticationCode2", &second),
            ],
        )
        .await;
        assert_eq!(
            status, row.status,
            "case {:?}: expected HTTP {}, got {status} {body}",
            row.case, row.status
        );
        if let Some(expected) = row.error {
            let code = error_code(&body);
            assert_eq!(
                code, expected,
                "case {:?}: expected {expected}, got {code:?}: {body}",
                row.case
            );
        }

        // What the account holds afterwards: the device is assigned exactly when the enable
        // was served, and the listing is where its user and enable date are reported.
        let (status, listed) = call(
            &router,
            "ListVirtualMFADevices",
            &[("AssignmentStatus", "Assigned")],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the account's devices must be listable, got {status} {listed}",
            row.case
        );
        let assigned = elements(&listed, "SerialNumber");
        if row.error.is_none() {
            assert_eq!(
                assigned,
                vec![serial_number.clone()],
                "case {:?}: expected the device to be assigned, got {assigned:?}",
                row.case
            );
            let user = element(&listed, "UserName");
            assert_eq!(
                user, USER,
                "case {:?}: expected the device to report its holder, got {user:?}: {listed}",
                row.case
            );
            assert!(
                listed.contains("<EnableDate>"),
                "case {:?}: expected an EnableDate on the enabled device, got {listed}",
                row.case
            );
        } else {
            assert!(
                assigned.is_empty(),
                "case {:?}: expected the device to stay unassigned, got {assigned:?}",
                row.case
            );
        }
    }
}

/// RULE: what the request ADDRESSES has to exist and be free — a serial the account holds no
/// device under and a user it does not hold are both the entity that does not exist, and a
/// device its holder already has is a resource that already exists.
#[tokio::test]
async fn an_absent_entity_and_an_enabled_device_are_refused_in_their_own_classes() {
    let router = router();
    given_user(&router, USER).await;
    given_user(&router, "second-operator").await;
    let (serial_number, seed) = given_device(&router, "operator-device").await;
    let step = current_step(SystemTime::now());
    let (first, second) = (pass_code(&seed, step), pass_code(&seed, step + 1));
    let absent_serial = format!("{serial_number}-absent");

    let rows: [(&str, &str, &str, u16, &str); 4] = [
        (
            "a serial the account holds no device under",
            USER,
            absent_serial.as_str(),
            404,
            "NoSuchEntity",
        ),
        (
            "a user the account does not hold",
            "nobody",
            serial_number.as_str(),
            404,
            "NoSuchEntity",
        ),
        (
            "the device its own holder already has",
            USER,
            serial_number.as_str(),
            409,
            "EntityAlreadyExists",
        ),
        (
            "the device another user already holds",
            "second-operator",
            serial_number.as_str(),
            409,
            "EntityAlreadyExists",
        ),
    ];

    // The first enable has to succeed: the two conflict rows are about a device that is
    // already enabled, and the two absent-entity rows are decided before it matters.
    let (status, body) = call(
        &router,
        "EnableMFADevice",
        &[
            ("UserName", USER),
            ("SerialNumber", &serial_number),
            ("AuthenticationCode1", &first),
            ("AuthenticationCode2", &second),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the device must be enabled for the first user, got {status} {body}"
    );

    for (case, user, serial, expected_status, expected_code) in rows {
        let (status, body) = call(
            &router,
            "EnableMFADevice",
            &[
                ("UserName", user),
                ("SerialNumber", serial),
                ("AuthenticationCode1", &first),
                ("AuthenticationCode2", &second),
            ],
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected {expected_code}, got {code:?}: {body}"
        );
    }
}
