// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for AppStream's user pool over the wire.
//!
//! Every case drives the awsJson1_1 front door through the shared
//! [`Emulator`](crate::stack_fleet_lifecycle::Emulator) harness, so what is exercised is
//! the dispatch → deserialize → handle → serialize walk.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one
//! recorded request:
//!
//! * creating a user allocates the `(AuthenticationType, UserName)` identity and
//!   persists it, so a later read reports that user with the ARN, the names and the
//!   creation instant the create stored, enabled and not yet confirmed;
//! * the identity is allocated at most once: a repeat create of an identity that exists
//!   asks for the user that is already there, so it answers the same empty success and
//!   changes nothing — however many callers race on it — while a repeat that names an
//!   attribute differing from the stored one is refused with
//!   `ResourceAlreadyExistsException` (400); and names differing only in capitalisation
//!   are different identities, because email addresses are case-sensitive;
//! * the user pool operations manage USERPOOL identities only, so any other
//!   authentication type is refused with `InvalidParameterCombinationException` (400);
//! * `RESEND` re-sends an existing user's welcome email, so naming the user's first or
//!   last name in the same request is refused as an incorrect combination;
//! * a read reports only stored users, in a stable order, and `MaxResults` sizes its
//!   page: following `NextToken` walks every stored user exactly once;
//! * a request outside a modelled constraint (the user name's length bounds, its
//!   character pattern, a page smaller than one record, a token this service never
//!   issued) is refused and persists nothing, and so is a user past the store's own
//!   ceiling.

use serde_json::{json, Value};

use crate::refusal_messages::refusal;
use crate::stack_fleet_lifecycle::{assert_error, Emulator};

/// The account every emulated resource belongs to.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// AWS's `ResourceAlreadyExistsException` sentence for a user pool username that is
/// already taken. It has no variable part, so the refusal echoes nothing back.
const USER_ALREADY_EXISTS_MESSAGE: &str =
    service_com_amazonaws_appstream_photonadminproxyservice::state::USER_ALREADY_EXISTS_MESSAGE;

/// The only authentication type the user pool operations accept, which both
/// `CreateUser.AuthenticationType` and `DescribeUsers.AuthenticationType` document as
/// "You must specify USERPOOL".
const USER_POOL: &str = "USERPOOL";

/// The other authentication types the model declares (`AuthenticationType`'s values,
/// kept as `AUTHENTICATION_TYPE_VALUES` in `src/ops/types/shared_types.rs`). None of
/// them names a user pool identity, so the user pool operations refuse every one.
const OTHER_AUTHENTICATION_TYPES: &[&str] = &["API", "AWS_AD", "SAML"];

/// The status of a user who has been created but has not confirmed the account by
/// setting a password, one of `User.Status`'s documented values
/// (`smithy_query(query="doc", shape="User", member="Status")`).
const STATUS_UNCONFIRMED: &str = "UNCONFIRMED";

/// The length bounds the model puts on `Username` (len 1..128).
const USER_NAME_MIN: usize = 1;
const USER_NAME_MAX: usize = 128;

/// One row of the creation table: the members the request carries.
struct Row {
    case: &'static str,
    user_name: String,
    first_name: Option<&'static str>,
    last_name: Option<&'static str>,
    message_action: Option<&'static str>,
}

/// The creation table. The rows differ along what the rule generalises over: the form
/// and size of the user name (ASCII email, tagged address, non-ASCII letters, and both
/// published length bounds), which optional attributes the request carries, and which
/// welcome-email action it asks for.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "both names, default welcome email",
            user_name: "first.last@example.com".to_string(),
            first_name: Some("Ada"),
            last_name: Some("Lovelace"),
            message_action: None,
        },
        Row {
            case: "no names, welcome email suppressed",
            user_name: "suppressed@example.com".to_string(),
            first_name: None,
            last_name: None,
            message_action: Some("SUPPRESS"),
        },
        Row {
            case: "first name only, tagged address",
            user_name: "tagged+label@example.com".to_string(),
            first_name: Some("Grace Brewster"),
            last_name: None,
            message_action: Some("SUPPRESS"),
        },
        Row {
            case: "last name only, non-ASCII user name",
            user_name: "ユーザー@example.com".to_string(),
            first_name: None,
            last_name: Some("Hopper-Murray"),
            message_action: None,
        },
        Row {
            case: "shortest user name the model allows",
            user_name: "a".repeat(USER_NAME_MIN),
            first_name: None,
            last_name: None,
            message_action: None,
        },
        Row {
            case: "longest user name the model allows",
            user_name: format!("{}@example.com", "a".repeat(USER_NAME_MAX - "@example.com".len())),
            first_name: Some("Katherine"),
            last_name: Some("Johnson"),
            message_action: Some("SUPPRESS"),
        },
        Row {
            case: "welcome email resent, no names",
            user_name: "resend@example.com".to_string(),
            first_name: None,
            last_name: None,
            message_action: Some("RESEND"),
        },
    ]
}

/// The `CreateUser` request a row asks for.
fn create_request(row: &Row) -> Value {
    let mut request = json!({
        "UserName": row.user_name,
        "AuthenticationType": USER_POOL,
    });
    if let Some(first_name) = row.first_name {
        request["FirstName"] = json!(first_name);
    }
    if let Some(last_name) = row.last_name {
        request["LastName"] = json!(last_name);
    }
    if let Some(message_action) = row.message_action {
        request["MessageAction"] = json!(message_action);
    }
    request
}

/// The users a `DescribeUsers` reports, and the pagination token it carries.
async fn describe(emulator: &Emulator, case: &str, request: Value) -> (Vec<Value>, Option<Value>) {
    let payload = emulator.ok(case, "DescribeUsers", request).await;
    let users = payload
        .get("Users")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a Users list, got {payload}"))
        .clone();
    (users, payload.get("NextToken").cloned())
}

/// Every user of the pool, in the order the read reports them.
async fn user_pool(emulator: &Emulator, case: &str) -> Vec<Value> {
    let (users, next_token) = describe(
        emulator,
        case,
        json!({ "AuthenticationType": USER_POOL }),
    )
    .await;
    assert!(
        next_token.is_none(),
        "case {case:?}: the whole pool fits in one page here, got {next_token:?}"
    );
    users
}

/// The `UserName` of a reported user.
fn user_name(case: &str, user: &Value) -> String {
    user.get("UserName")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a UserName, got {user}"))
        .to_string()
}

/// The user a row must be reported as, given the `CreatedTime` the create stamped.
fn expected_user(row: &Row, created_time: &Value) -> Value {
    let mut expected = json!({
        "Arn": format!(
            "arn:aws:appstream:us-east-1:{ACCOUNT}:user/userpool/{}",
            row.user_name
        ),
        "UserName": row.user_name,
        "Enabled": true,
        "Status": STATUS_UNCONFIRMED,
        "AuthenticationType": USER_POOL,
        "CreatedTime": created_time,
    });
    if let Some(first_name) = row.first_name {
        expected["FirstName"] = json!(first_name);
    }
    if let Some(last_name) = row.last_name {
        expected["LastName"] = json!(last_name);
    }
    expected
}

/// The `CreatedTime` of a reported user, asserted to be an instant and not a placeholder.
fn created_time(case: &str, user: &Value) -> Value {
    let reported = user
        .get("CreatedTime")
        .unwrap_or_else(|| panic!("case {case:?}: expected a CreatedTime, got {user}"));
    let seconds = reported
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: CreatedTime must be an instant, got {reported}"));
    assert!(
        seconds > 0.0,
        "case {case:?}: CreatedTime must be after the epoch, got {seconds}"
    );
    reported.clone()
}

// ---------------------------------------------------------------------------
// Creating a user, and reading it back
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_user_persists_the_identity_a_later_read_reports() {
    for row in rows() {
        let case = row.case;
        let emulator = Emulator::new();

        // Nothing has been created yet, so the pool reports no user at all.
        let before = user_pool(&emulator, case).await;
        assert!(
            before.is_empty(),
            "case {case:?}: an account with no users must report none, got {before:?}"
        );

        let created = emulator.ok(case, "CreateUser", create_request(&row)).await;
        assert_eq!(
            created,
            json!({}),
            "case {case:?}: the modelled result carries no members, got {created}"
        );

        let users = user_pool(&emulator, case).await;
        assert_eq!(
            users.len(),
            1,
            "case {case:?}: exactly the created user must be reported, got {users:?}"
        );
        let stamped = created_time(case, &users[0]);
        assert_eq!(
            users[0],
            expected_user(&row, &stamped),
            "case {case:?}: the read must render the record the create persisted, got {:?}",
            users[0]
        );

        // Reading again reports the same user: the first read changed nothing, and the
        // creation instant belongs to the record rather than to the call that read it.
        let again = user_pool(&emulator, case).await;
        assert_eq!(
            again, users,
            "case {case:?}: a read must not change what a later caller sees, got {again:?}"
        );
    }
}

#[tokio::test]
async fn a_repeat_create_of_an_identity_that_exists_is_idempotent() {
    // Rows: what the repeat request carries beside the identity, and whether it asks for
    // the user that is already there. A repeat that asks for nothing different is
    // answered with the same empty success; one that names an attribute differing from
    // the stored value asks for a different user under a name that is taken, and is
    // refused. Either way the stored user is left exactly as it was.
    for (case, repeat_extra, succeeds) in [
        ("identical request", json!({ "FirstName": "Ada", "LastName": "Lovelace" }), true),
        ("only the identity", json!({}), true),
        ("welcome email resent", json!({ "MessageAction": "RESEND" }), true),
        ("one name repeated", json!({ "LastName": "Lovelace" }), true),
        ("a different first name", json!({ "FirstName": "Augusta" }), false),
        ("a different last name", json!({ "LastName": "Byron" }), false),
        (
            "both names different",
            json!({ "FirstName": "Augusta", "LastName": "Byron" }),
            false,
        ),
    ] {
        let row = Row {
            case,
            user_name: "repeat@example.com".to_string(),
            first_name: Some("Ada"),
            last_name: Some("Lovelace"),
            message_action: Some("SUPPRESS"),
        };
        let emulator = Emulator::new();
        emulator.ok(case, "CreateUser", create_request(&row)).await;
        let first = user_pool(&emulator, case).await;

        let mut repeat = json!({
            "UserName": row.user_name,
            "AuthenticationType": USER_POOL,
        });
        for (member, value) in repeat_extra.as_object().expect("the row carries members") {
            repeat[member] = value.clone();
        }

        if succeeds {
            let answer = emulator.ok(case, "CreateUser", repeat).await;
            assert_eq!(
                answer,
                json!({}),
                "case {case:?}: a repeat create answers the same empty result, got {answer}"
            );
        } else {
            let message = refusal(
                &emulator,
                case,
                "CreateUser",
                repeat,
                400,
                "ResourceAlreadyExistsException",
            )
            .await;
            assert_eq!(
                message, USER_ALREADY_EXISTS_MESSAGE,
                "case {case:?}: expected message {USER_ALREADY_EXISTS_MESSAGE:?}, got {message:?}"
            );
            assert!(
                !message.contains(row.user_name.as_str()),
                "case {case:?}: the sentence states that the username is taken without \
                 echoing it, got {message:?}"
            );
        }

        let after = user_pool(&emulator, case).await;
        assert_eq!(
            after, first,
            "case {case:?}: the identity is allocated once, so the second call must leave \
             the stored user exactly as it was, got {after:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_creates_of_one_identity_all_succeed_and_allocate_it_once() {
    // Rows: how many callers race on the same identity, and what each of them carries.
    // However many of them arrive at once, each is answered 200 and the pool holds one
    // user for that name afterwards.
    for (racers, extra) in [(2_usize, json!({})), (6, json!({ "MessageAction": "SUPPRESS" })), (16, json!({}))] {
        let case = format!("{racers} racers");
        let emulator = std::sync::Arc::new(Emulator::new());
        let mut request = json!({
            "UserName": "racer@example.com",
            "AuthenticationType": USER_POOL,
        });
        for (member, value) in extra.as_object().expect("the row carries members") {
            request[member] = value.clone();
        }

        let mut racing = Vec::new();
        for _ in 0..racers {
            let emulator = std::sync::Arc::clone(&emulator);
            let request = request.clone();
            racing.push(tokio::spawn(async move {
                emulator.call("CreateUser", request).await
            }));
        }
        for racer in racing {
            let (status, payload) = racer.await.expect("the racing call completes");
            assert_eq!(
                status, 200,
                "case {case:?}: every racer on one identity must succeed, got {status} {payload}"
            );
            assert_eq!(
                payload,
                json!({}),
                "case {case:?}: every racer gets the same empty result, got {payload}"
            );
        }

        let users = user_pool(&emulator, &case).await;
        assert_eq!(
            users.len(),
            1,
            "case {case:?}: the identity must be allocated exactly once, got {users:?}"
        );
        assert_eq!(
            users[0].get("Enabled"),
            Some(&json!(true)),
            "case {case:?}: the user the racers created must be enabled, got {:?}",
            users[0]
        );
    }
}

#[tokio::test]
async fn user_names_differing_only_in_capitalisation_are_different_users() {
    // Rows: pairs of names that differ only in case. Email addresses are case-sensitive,
    // so each spelling is its own identity with its own ARN.
    for (lower, other) in [
        ("user@example.com", "User@example.com"),
        ("ada.lovelace@example.com", "Ada.Lovelace@EXAMPLE.com"),
    ] {
        let case = format!("{lower} vs {other}");
        let emulator = Emulator::new();

        for name in [lower, other] {
            emulator
                .ok(
                    &case,
                    "CreateUser",
                    json!({ "UserName": name, "AuthenticationType": USER_POOL }),
                )
                .await;
        }

        let users = user_pool(&emulator, &case).await;
        let mut reported: Vec<String> = users
            .iter()
            .map(|user| user_name(&case, user))
            .collect();
        reported.sort();
        let mut expected = vec![lower.to_string(), other.to_string()];
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {case:?}: both spellings must be their own user, got {users:?}"
        );
        for user in &users {
            let name = user_name(&case, user);
            assert_eq!(
                user.get("Arn").and_then(Value::as_str),
                Some(
                    format!("arn:aws:appstream:us-east-1:{ACCOUNT}:user/userpool/{name}").as_str()
                ),
                "case {case:?}: each user's ARN must name that user, got {user}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

#[tokio::test]
async fn only_user_pool_identities_are_managed_by_these_operations() {
    for authentication_type in OTHER_AUTHENTICATION_TYPES {
        let case = format!("AuthenticationType {authentication_type}");
        let emulator = Emulator::new();

        assert_error(
            &emulator,
            &case,
            "CreateUser",
            json!({
                "UserName": "elsewhere@example.com",
                "AuthenticationType": authentication_type,
            }),
            400,
            "InvalidParameterCombinationException",
        )
        .await;
        assert_error(
            &emulator,
            &case,
            "DescribeUsers",
            json!({ "AuthenticationType": authentication_type }),
            400,
            "InvalidParameterCombinationException",
        )
        .await;

        // The refusals changed nothing: the user pool is still empty, and creating a
        // user pool identity still works.
        let users = user_pool(&emulator, &case).await;
        assert!(
            users.is_empty(),
            "case {case:?}: a refused create must persist nothing, got {users:?}"
        );
        emulator
            .ok(
                &case,
                "CreateUser",
                json!({ "UserName": "pooled@example.com", "AuthenticationType": USER_POOL }),
            )
            .await;
        assert_eq!(
            user_pool(&emulator, &case).await.len(),
            1,
            "case {case:?}: a user pool create must still allocate its identity"
        );
    }
}

#[tokio::test]
async fn resending_the_welcome_email_cannot_also_set_the_users_names() {
    // Rows: which name members the RESEND request carries. Each is a combination the
    // operation cannot satisfy, so each is refused and persists nothing.
    for (case, extra) in [
        ("FirstName", json!({ "FirstName": "Ada" })),
        ("LastName", json!({ "LastName": "Lovelace" })),
        (
            "both names",
            json!({ "FirstName": "Ada", "LastName": "Lovelace" }),
        ),
    ] {
        let emulator = Emulator::new();
        let mut request = json!({
            "UserName": "resend@example.com",
            "AuthenticationType": USER_POOL,
            "MessageAction": "RESEND",
        });
        for (member, value) in extra.as_object().expect("the row carries members") {
            request[member] = value.clone();
        }

        assert_error(
            &emulator,
            case,
            "CreateUser",
            request,
            400,
            "InvalidParameterCombinationException",
        )
        .await;

        let users = user_pool(&emulator, case).await;
        assert!(
            users.is_empty(),
            "case {case:?}: a refused create must persist nothing, got {users:?}"
        );
    }
}

#[tokio::test]
async fn a_user_name_outside_the_models_constraints_is_refused() {
    // Rows: user names outside the published bounds — one shorter than the minimum, one
    // past the maximum — and one outside the character pattern.
    let emulator = Emulator::new();
    for (case, name) in [
        ("shorter than the minimum", String::new()),
        (
            "one past the maximum",
            "a".repeat(USER_NAME_MAX + 1 - "@example.com".len()) + "@example.com",
        ),
        (
            "a space, outside the pattern",
            "first last@example.com".to_string(),
        ),
    ] {
        assert_error(
            &emulator,
            case,
            "CreateUser",
            json!({ "UserName": name, "AuthenticationType": USER_POOL }),
            400,
            "ValidationException",
        )
        .await;
    }

    let users = user_pool(&emulator, "constraints").await;
    assert!(
        users.is_empty(),
        "case \"constraints\": a refused create must persist nothing, got {users:?}"
    );
}

// ---------------------------------------------------------------------------
// Paging the read
// ---------------------------------------------------------------------------

/// The user names the pagination cases create: enough records that every page size below
/// cuts the collection somewhere.
fn paged_user_names() -> Vec<String> {
    (1..=7).map(|index| format!("user-{index:02}@example.com")).collect()
}

#[tokio::test]
async fn following_next_token_reports_every_user_exactly_once() {
    let emulator = Emulator::new();
    for name in paged_user_names() {
        emulator
            .ok(
                "setup",
                "CreateUser",
                json!({ "UserName": name, "AuthenticationType": USER_POOL }),
            )
            .await;
    }
    let mut expected = paged_user_names();
    expected.sort();

    // Rows: the page size the request asks for — sizes that cut the collection, one that
    // holds it exactly, one larger than it, and a request that names no size at all.
    for max_results in [Some(1_i64), Some(2), Some(3), Some(7), Some(50), None] {
        let case = format!("MaxResults {max_results:?}");
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<Value> = None;
        loop {
            let mut request = json!({ "AuthenticationType": USER_POOL });
            if let Some(max_results) = max_results {
                request["MaxResults"] = json!(max_results);
            }
            if let Some(token) = &next_token {
                request["NextToken"] = token.clone();
            }
            let (users, token) = describe(&emulator, &case, request).await;
            if let Some(max_results) = max_results {
                assert!(
                    i64::try_from(users.len()).expect("a page fits in an i64") <= max_results,
                    "case {case:?}: a page must hold at most {max_results} users, got {}",
                    users.len()
                );
            }
            seen.extend(users.iter().map(|user| user_name(&case, user)));
            match token {
                Some(token) => next_token = Some(token),
                None => break,
            }
        }
        assert_eq!(
            seen, expected,
            "case {case:?}: following NextToken must report every user exactly once, in a \
             stable order, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_page_the_read_cannot_answer_with_is_refused() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "setup",
            "CreateUser",
            json!({ "UserName": "paged@example.com", "AuthenticationType": USER_POOL }),
        )
        .await;

    // Rows: page sizes that cannot hold a record. Each is refused rather than answered
    // with an empty page that silently drops the stored users.
    for max_results in [0_i64, -1, i64::from(i32::MIN)] {
        let case = format!("MaxResults {max_results}");
        assert_error(
            &emulator,
            &case,
            "DescribeUsers",
            json!({ "AuthenticationType": USER_POOL, "MaxResults": max_results }),
            400,
            "ValidationException",
        )
        .await;
    }

    // Rows: tokens a client could hand-craft. The cursor is an opaque encoding this
    // service mints, so none of these is a token it issued.
    for token in [
        "paged@example.com",
        "abc",
        "zzzz",
        "ff",
    ] {
        let case = format!("NextToken {token:?}");
        assert_error(
            &emulator,
            &case,
            "DescribeUsers",
            json!({ "AuthenticationType": USER_POOL, "NextToken": token }),
            400,
            "ValidationException",
        )
        .await;
    }

    // Every refusal left the stored user readable.
    let users = user_pool(&emulator, "after refusals").await;
    assert_eq!(
        users.len(),
        1,
        "case \"after refusals\": the stored user must still be reported, got {users:?}"
    );
}

// ---------------------------------------------------------------------------
// The store's ceiling
// ---------------------------------------------------------------------------

/// The emulator's memory ceiling, read from the crate so the test tracks the constant
/// rather than a copy of its value.
const CEILING: usize =
    service_com_amazonaws_appstream_photonadminproxyservice::state::MAX_RECORDS_PER_STORE;

#[tokio::test]
async fn the_user_store_refuses_a_user_past_its_ceiling() {
    let case = "users";
    let emulator = Emulator::new();
    for index in 0..CEILING {
        emulator
            .ok(
                case,
                "CreateUser",
                json!({
                    "UserName": format!("u{index:05}@example.com"),
                    "AuthenticationType": USER_POOL,
                }),
            )
            .await;
    }

    assert_error(
        &emulator,
        case,
        "CreateUser",
        json!({
            "UserName": "one-too-many@example.com",
            "AuthenticationType": USER_POOL,
        }),
        400,
        "LimitExceededException",
    )
    .await;

    // An identity already held allocates nothing, so a repeat create of one is answered
    // with the same empty success at the ceiling as below it.
    emulator
        .ok(
            case,
            "CreateUser",
            json!({
                "UserName": "u00000@example.com",
                "AuthenticationType": USER_POOL,
            }),
        )
        .await;
}
