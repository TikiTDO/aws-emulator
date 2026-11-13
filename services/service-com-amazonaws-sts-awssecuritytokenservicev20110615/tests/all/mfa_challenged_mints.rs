// SPDX-License-Identifier: CC0-1.0

//! What an MFA CHALLENGE a mint presents proves, and what the session it mints carries.
//!
//! The rule under test: **a request that presents `SerialNumber` with `TokenCode` is
//! authenticated against the MFA device the account holds for the CALLING identity — the code
//! has to be one that device emits — and the session a satisfied challenge mints is
//! MFA-authenticated, which is what the global `aws:MultiFactorAuthPresent` condition key
//! answers `true` for.** "MFA-enabled IAM users must call `GetSessionToken` and submit an MFA
//! code that is associated with their MFA device … an incorrect MFA code causes the API to
//! return an access denied error" (`GetSessionToken`), and the same challenge decides
//! `sts:AssumeRole` ("if the role being assumed requires MFA and if the `TokenCode` value is
//! missing or expired, the `AssumeRole` call returns an 'access denied' error",
//! `AssumeRoleRequest$TokenCode`). A session minted with NO challenge is not MFA-authenticated,
//! and the key answers nothing at all for a long-term access key, which is no session.
//!
//! The device is provisioned and enabled through IAM's own operations
//! (`CreateVirtualMFADevice` + `EnableMFADevice`) over the same shared engine, so the seed the
//! challenge is decided against is the one that account really holds; the suite computes the
//! pass code itself, from RFC 6238's definition — the arithmetic the holder of the device does.
//!
//! The rows differ along what the rule generalises over: which code is presented (the one the
//! device emits, another step's, a code with no serial beside it), which serial it is presented
//! against (the device's own, one the account holds no device under), whose device it is, and
//! which mint is called.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use service_com_amazonaws_iam_awsidentitymanagementv20100508 as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615 as sts;
use smithy_server_runtime::MultiServiceRouter;
use sts::state::ComAmazonawsStsAwssecuritytokenservicev20110615State as StsState;

use super::sts_sessions::{
    call, element, emulator_account, emulator_with_iam, query, seed_demo_role,
};

/// The IAM user whose device every row's challenge is decided against.
const USER: &str = "mfa-operator";

/// Its long-term access key id, which the mints are signed with.
const USER_KEY_ID: &str = "AKIAEMULATORMFAUSER1";

/// How long one time step of a TOTP pass code lasts: RFC 6238's recommended default, which an
/// authenticator app provisioned from an `otpauth://totp/` URI naming no period uses.
const TIME_STEP: Duration = Duration::from_secs(30);

/// The access key id standing for the account's own published credential, which enrols the
/// device: neither STS nor IAM holds a record of it, so it speaks for the account itself — the
/// position an administrator provisioning a user's device is in.
const ACCOUNT_KEY: &str = "AKIDEMULATOR";

/// The IAM user and its access key, as `iam:CreateUser` and `iam:CreateAccessKey` record them,
/// in the IAM store of the same shared engine the mints read the caller from.
fn given_user(engine: &engine::InMemoryEngine) -> String {
    let arn = format!("arn:aws:iam::{}:user/{USER}", emulator_account());
    let store = engine.state::<iam::ComAmazonawsIamAwsidentitymanagementv20100508State>();
    let mut store = store.lock();
    store.users.insert(
        iam::state::name_key(USER),
        iam::ops::types::User {
            path: "/".to_string(),
            user_name: USER.to_string(),
            user_id: "AIDAEMULATORMFAUSER01".to_string(),
            arn: arn.clone(),
            create_date: aws_smithy_types::DateTime::from_secs(0),
            password_last_used: None,
            permissions_boundary: None,
            tags: None,
        },
    );
    store.access_keys.insert(
        USER_KEY_ID.to_string(),
        iam::state::AccessKeyRecord {
            user_key: iam::state::name_key(USER),
            status: iam::state::ACCESS_KEY_ACTIVE_STATUS.to_string(),
            create_date: aws_smithy_types::DateTime::from_secs(0),
        },
    );
    arn
}

/// Provision a virtual MFA device through IAM's own `CreateVirtualMFADevice` and enable it for
/// `user_name` with the two codes it emits — the enrolment a holder of the device performs.
/// Answers the device's serial number and its base32 seed.
async fn given_enabled_device(
    router: &MultiServiceRouter,
    user_name: &str,
    device_name: &str,
) -> (String, String) {
    let (status, body) = query(
        router,
        ACCOUNT_KEY,
        "iam",
        "2010-05-08",
        "CreateVirtualMFADevice",
        &[("VirtualMFADeviceName", device_name.to_string())],
    )
    .await;
    assert_eq!(
        status, 200,
        "the device {device_name:?} must be provisioned, got {status} {body}"
    );
    let serial_number = element(&body, "SerialNumber")
        .unwrap_or_else(|| panic!("the create must answer a SerialNumber, got {body}"));
    let encoded = element(&body, "Base32StringSeed")
        .unwrap_or_else(|| panic!("the create must answer the seed, got {body}"));
    // `Base32StringSeed` is a blob, so the wire carries it base64-encoded.
    let seed = String::from_utf8(
        data_encoding::BASE64
            .decode(encoded.as_bytes())
            .unwrap_or_else(|error| panic!("the seed must be base64, got {encoded:?}: {error}")),
    )
    .expect("the seed is base32 text");
    let step = current_step(SystemTime::now());
    let (status, body) = query(
        router,
        ACCOUNT_KEY,
        "iam",
        "2010-05-08",
        "EnableMFADevice",
        &[
            ("UserName", user_name.to_string()),
            ("SerialNumber", serial_number.clone()),
            ("AuthenticationCode1", pass_code(&seed, step)),
            ("AuthenticationCode2", pass_code(&seed, step + 1)),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the device must be enabled for {user_name:?}, got {status} {body}"
    );
    (serial_number, seed)
}

/// The pass code a device provisioned with `seed` emits in the time step `step`, computed here
/// rather than asked of either service (see the module docs): RFC 6238's TOTP over RFC 4226's
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
    format!("{:06}", truncated % 1_000_000)
}

/// The time step the instant `now` falls in.
fn current_step(now: SystemTime) -> u64 {
    now.duration_since(UNIX_EPOCH)
        .expect("the clock is past the epoch")
        .as_secs()
        / TIME_STEP.as_secs()
}

/// Whether the session the registry holds under `access_key_id` is MFA-authenticated, as every
/// later decision about that credential reads it
/// (`sts::state::Caller::multi_factor_auth_present`).
fn mfa_present(engine: &engine::InMemoryEngine, access_key_id: &str) -> Option<bool> {
    engine
        .state::<StsState>()
        .lock()
        .session(access_key_id)
        .map(|session| session.mfa_authenticated)
}

/// RULE: `GetSessionToken` mints an MFA-AUTHENTICATED session exactly when the challenge it
/// presents is one the caller's own device answers, and a session minted with no challenge
/// carries no MFA.
#[tokio::test]
async fn a_satisfied_challenge_mints_an_mfa_authenticated_session() {
    let (router, engine) = emulator_with_iam();
    given_user(&engine);
    let (serial_number, seed) = given_enabled_device(&router, USER, "operator-device").await;
    let step = current_step(SystemTime::now());

    for (case, params, mfa) in [
        (
            "no challenge at all",
            Vec::new(),
            Some(false),
        ),
        (
            "the code the device emits",
            vec![
                ("SerialNumber", serial_number.clone()),
                ("TokenCode", pass_code(&seed, step)),
            ],
            Some(true),
        ),
    ] {
        let (status, body) = call(&router, USER_KEY_ID, "GetSessionToken", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the mint to succeed, got {status} {body}"
        );
        let minted = element(&body, "AccessKeyId")
            .unwrap_or_else(|| panic!("case {case:?}: expected a credential, got {body}"));
        let present = mfa_present(&engine, &minted);
        assert_eq!(
            present, mfa,
            "case {case:?}: expected the session's MFA state to be {mfa:?}, got {present:?}"
        );
    }
}

/// One row of the refusal table: the case, the challenge parameters the mint carries, and the
/// sentence STS refuses it with.
type Refused<'a> = (&'a str, Vec<(&'a str, String)>, &'a str);

/// RULE: a challenge the device does NOT answer mints nothing — the code has to be one that
/// device emits, presented against that device's own serial, by the identity it is enabled for.
#[tokio::test]
async fn a_challenge_the_device_does_not_answer_mints_nothing() {
    let (router, engine) = emulator_with_iam();
    given_user(&engine);
    let (serial_number, seed) = given_enabled_device(&router, USER, "operator-device").await;
    let step = current_step(SystemTime::now());
    let absent_serial = format!("{serial_number}-absent");

    let rows: [Refused<'_>; 4] = [
        (
            "a code from another time step",
            vec![
                ("SerialNumber", serial_number.clone()),
                ("TokenCode", pass_code(&seed, step + 500)),
            ],
            "MultiFactorAuthentication failed with invalid MFA one time pass code.",
        ),
        (
            "a serial the account holds no device under",
            vec![
                ("SerialNumber", absent_serial.clone()),
                ("TokenCode", pass_code(&seed, step)),
            ],
            "MultiFactorAuthentication failed, unable to validate MFA code.  Please verify your \
             MFA serial number is valid and associated with this user.",
        ),
        (
            "a code with no serial beside it",
            vec![("TokenCode", pass_code(&seed, step))],
            "MultiFactorAuthentication failed, unable to validate MFA code.  Please verify your \
             MFA serial number is valid and associated with this user.",
        ),
        (
            "a serial with no code beside it",
            vec![("SerialNumber", serial_number.clone())],
            "MultiFactorAuthentication failed, unable to validate MFA code.  Please verify your \
             MFA serial number is valid and associated with this user.",
        ),
    ];

    let held = engine.state::<StsState>().lock().sessions.len();
    for (case, params, expected) in rows {
        let (status, body) = call(&router, USER_KEY_ID, "GetSessionToken", &params).await;
        assert_eq!(
            status, 403,
            "case {case:?}: expected HTTP 403, got {status} {body}"
        );
        let message = element(&body, "Message").unwrap_or_default();
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        let now_held = engine.state::<StsState>().lock().sessions.len();
        assert_eq!(
            now_held, held,
            "case {case:?}: expected no session to be minted, got {now_held} where {held} were \
             held"
        );
    }
}

/// RULE, on the other mint that takes a challenge: `AssumeRole` decides it the same way, so the
/// role session a satisfied challenge mints is MFA-authenticated and one minted without a
/// challenge is not.
#[tokio::test]
async fn assume_role_carries_the_challenge_into_the_role_session() {
    let (router, engine) = emulator_with_iam();
    given_user(&engine);
    let (role_arn, _role_id) = seed_demo_role(&engine);
    let (serial_number, seed) = given_enabled_device(&router, USER, "operator-device").await;
    let step = current_step(SystemTime::now());

    for (case, challenge, mfa) in [
        ("no challenge at all", Vec::new(), Some(false)),
        (
            "the code the device emits",
            vec![
                ("SerialNumber", serial_number.clone()),
                ("TokenCode", pass_code(&seed, step)),
            ],
            Some(true),
        ),
    ] {
        let mut params = vec![
            ("RoleArn", role_arn.clone()),
            ("RoleSessionName", "operator".to_string()),
        ];
        params.extend(challenge);
        let (status, body) = call(&router, USER_KEY_ID, "AssumeRole", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the assume to succeed, got {status} {body}"
        );
        let minted = element(&body, "AccessKeyId")
            .unwrap_or_else(|| panic!("case {case:?}: expected a credential, got {body}"));
        let present = mfa_present(&engine, &minted);
        assert_eq!(
            present, mfa,
            "case {case:?}: expected the session's MFA state to be {mfa:?}, got {present:?}"
        );
    }
}
