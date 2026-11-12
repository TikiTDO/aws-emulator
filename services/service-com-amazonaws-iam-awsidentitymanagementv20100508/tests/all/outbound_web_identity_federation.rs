// SPDX-License-Identifier: CC0-1.0

//! The account's OUTBOUND web identity federation — `EnableOutboundWebIdentityFederation`,
//! `GetOutboundWebIdentityFederationInfo` and `DisableOutboundWebIdentityFederation` — end
//! to end over the awsQuery wire.
//!
//! RULE: the three actions are bound operations of this service, and all three read and
//! write ONE account-level record: the feature's flag plus the issuer URL the first enable
//! minted. A call that needs the feature in the state it is not in is refused with the
//! modelled error of the state the account IS in — `FeatureDisabled` (HTTP 404) while the
//! feature is off, `FeatureEnabled` (HTTP 409) while it is on — leaving the record exactly
//! as it was, and every served call reports that record rather than a value of its own.
//!
//! Each test is a TABLE whose rows differ along what the rule generalises over: which of
//! the three actions is called, which of the three states the account is in (never enabled,
//! enabled, enabled and disabled again), how many times an action is repeated from one
//! state, and — for the issuer — which account and which served endpoint it is derived for.
//! No single request is the specification.
//!
//! The awsQuery harness (`router_with_engine`, `call_signed_by`, `element`, `elements`,
//! `error_code`) is the one `tests/all/user_lifecycle.rs` owns and every suite in this crate
//! shares.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;

use crate::identity_policy_enforcement::{principal, statement};
use crate::user_lifecycle::{
    account_id, assert_access_denied_message, call_signed_by, element, elements, error_code,
    router_with_engine,
};

/// The awsQuery error code `FeatureDisabledException` carries
/// (`smithy_query(query="operation", operation="GetOutboundWebIdentityFederationInfo")` ->
/// `awsQueryError {code: FeatureDisabled, httpResponseCode: 404}`).
const FEATURE_DISABLED_CODE: &str = "FeatureDisabled";

/// The status that refusal carries, from the same `awsQueryError` trait.
const FEATURE_DISABLED_STATUS: u16 = 404;

/// The awsQuery error code `FeatureEnabledException` carries
/// (`smithy_query(query="operation", operation="EnableOutboundWebIdentityFederation")` ->
/// `awsQueryError {code: FeatureEnabled, httpResponseCode: 409}`).
const FEATURE_ENABLED_CODE: &str = "FeatureEnabled";

/// The status that refusal carries, from the same `awsQueryError` trait.
const FEATURE_ENABLED_STATUS: u16 = 409;

/// How many hexadecimal digits the opaque segment of the issuer URL carries.
const ISSUER_SEGMENT_DIGITS: usize = 32;

/// The three actions, named so a row reads as the action it is about.
const ENABLE: &str = "EnableOutboundWebIdentityFederation";
const INFO: &str = "GetOutboundWebIdentityFederationInfo";
const DISABLE: &str = "DisableOutboundWebIdentityFederation";

/// One call of `action`, presented as signed by `akid` when a row names one. All three
/// operations take no input, so a call carries no form parameter beyond the action.
async fn federation_call(
    router: &MultiServiceRouter,
    akid: Option<&str>,
    action: &str,
) -> (u16, String) {
    call_signed_by(router, akid, action, &[]).await
}

/// Whether the account has the feature ON, read straight off this service's own store:
/// what a refused call must leave untouched.
fn feature_is_on(engine: &engine::InMemoryEngine) -> bool {
    let store = state::service_state(engine).expect("the IAM store");
    let held = store
        .lock()
        .served_outbound_web_identity_federation()
        .is_some();
    held
}

/// The issuer the account holds, whatever the flag stands at, or `None` for an account that
/// never enabled the feature.
fn held_issuer(engine: &engine::InMemoryEngine) -> Option<String> {
    let store = state::service_state(engine).expect("the IAM store");
    let held = store
        .lock()
        .outbound_web_identity_federation
        .as_ref()
        .map(|federation| federation.issuer_identifier.clone());
    held
}

/// The whole message a refusal naming the account's state carries, for each of the three
/// states a call can be refused in. The fixed text is spelled out here rather than read
/// from the handler, so a change of wording fails this suite instead of following it; the
/// only variable part is the account the call was served for.
fn disabled_message() -> String {
    format!(
        "Outbound identity federation is disabled for account {}",
        account_id()
    )
}

/// See [`disabled_message`]: the DISABLE of an account that has the feature off already.
fn already_disabled_message() -> String {
    format!(
        "Outbound identity federation is already disabled for account {}",
        account_id()
    )
}

/// See [`disabled_message`]: the ENABLE of an account that has the feature on already.
fn already_enabled_message() -> String {
    format!(
        "Outbound identity federation is already enabled for account {}",
        account_id()
    )
}

/// Which of the three states a row puts the account in before it calls, and the calls that
/// put it there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Held {
    /// The account has never enabled the feature.
    NeverEnabled,
    /// The feature is on.
    Enabled,
    /// The feature was enabled and disabled again, so the account holds an issuer with the
    /// flag cleared.
    Disabled,
}

impl Held {
    /// Put a fresh account in this state, through the service's own operations.
    async fn reach(self, router: &MultiServiceRouter) {
        match self {
            Self::NeverEnabled => {}
            Self::Enabled => {
                let (status, body) = federation_call(router, None, ENABLE).await;
                assert_eq!(
                    status, 200,
                    "the fixture enable must succeed, got {status}: {body}"
                );
            }
            Self::Disabled => {
                let (status, body) = federation_call(router, None, ENABLE).await;
                assert_eq!(
                    status, 200,
                    "the fixture enable must succeed, got {status}: {body}"
                );
                let (status, body) = federation_call(router, None, DISABLE).await;
                assert_eq!(
                    status, 200,
                    "the fixture disable must succeed, got {status}: {body}"
                );
            }
        }
    }
}

/// What a row requires the call to answer.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Answer {
    /// HTTP 200: the state the account is in admits the call.
    Served,
    /// A refusal: the status, the awsQuery code and the whole message.
    Refused {
        status: u16,
        code: &'static str,
        message: String,
    },
}

/// One row of the state table: the state the account is in, the action called, and the
/// answer the rule requires.
struct Row {
    case: &'static str,
    held: Held,
    action: &'static str,
    answer: Answer,
    /// Whether the feature is ON after the call, which is what shows a refusal left the
    /// record alone and a served call moved it.
    on_after: bool,
}

/// The rows: every one of the three actions from every one of the three states — the whole
/// cross product, so the rule is asserted over the class and not over the one sequence the
/// defect was reported from.
fn state_rows() -> Vec<Row> {
    vec![
        Row {
            case: "enable an account that never enabled it",
            held: Held::NeverEnabled,
            action: ENABLE,
            answer: Answer::Served,
            on_after: true,
        },
        Row {
            case: "read an account that never enabled it",
            held: Held::NeverEnabled,
            action: INFO,
            answer: Answer::Refused {
                status: FEATURE_DISABLED_STATUS,
                code: FEATURE_DISABLED_CODE,
                message: disabled_message(),
            },
            on_after: false,
        },
        Row {
            case: "disable an account that never enabled it",
            held: Held::NeverEnabled,
            action: DISABLE,
            answer: Answer::Refused {
                status: FEATURE_DISABLED_STATUS,
                code: FEATURE_DISABLED_CODE,
                message: already_disabled_message(),
            },
            on_after: false,
        },
        Row {
            case: "enable an account that already has it on",
            held: Held::Enabled,
            action: ENABLE,
            answer: Answer::Refused {
                status: FEATURE_ENABLED_STATUS,
                code: FEATURE_ENABLED_CODE,
                message: already_enabled_message(),
            },
            on_after: true,
        },
        Row {
            case: "read an account that has it on",
            held: Held::Enabled,
            action: INFO,
            answer: Answer::Served,
            on_after: true,
        },
        Row {
            case: "disable an account that has it on",
            held: Held::Enabled,
            action: DISABLE,
            answer: Answer::Served,
            on_after: false,
        },
        Row {
            case: "enable an account that disabled it again",
            held: Held::Disabled,
            action: ENABLE,
            answer: Answer::Served,
            on_after: true,
        },
        Row {
            case: "read an account that disabled it again",
            held: Held::Disabled,
            action: INFO,
            answer: Answer::Refused {
                status: FEATURE_DISABLED_STATUS,
                code: FEATURE_DISABLED_CODE,
                message: disabled_message(),
            },
            on_after: false,
        },
        Row {
            case: "disable an account that disabled it again",
            held: Held::Disabled,
            action: DISABLE,
            answer: Answer::Refused {
                status: FEATURE_DISABLED_STATUS,
                code: FEATURE_DISABLED_CODE,
                message: already_disabled_message(),
            },
            on_after: false,
        },
    ]
}

/// RULE: each of the three actions is a bound operation of this service, so a call reaches
/// a handler and is answered with that operation's own response or one of its modelled
/// refusals — never with the dispatcher's `InvalidAction` for an action the service does
/// not have.
#[tokio::test]
async fn the_three_actions_are_routed_rather_than_answered_as_unknown_actions() {
    // One row per action, each stating the answer that action's own handler gives a fresh
    // account — the enable serves, the other two are refused `FeatureDisabled` — so the row
    // fails both for an unrouted action and for one answered by the wrong handler.
    let rows = [
        (ENABLE, 200, ""),
        (INFO, FEATURE_DISABLED_STATUS, FEATURE_DISABLED_CODE),
        (DISABLE, FEATURE_DISABLED_STATUS, FEATURE_DISABLED_CODE),
    ];
    for (action, expected_status, expected_code) in rows {
        let (router, _engine) = router_with_engine();
        let (status, body) = federation_call(&router, None, action).await;
        assert!(
            !body.contains("InvalidAction"),
            "case {action:?}: expected the action to be routed, got {status}: {body}"
        );
        assert_eq!(
            status, expected_status,
            "case {action:?}: expected {expected_status}, got {status}: {body}"
        );
        if expected_code.is_empty() {
            assert!(
                !body.contains("<Code>"),
                "case {action:?}: expected a served response, got {body}"
            );
        } else {
            assert_eq!(
                error_code(&body),
                expected_code,
                "case {action:?}: expected the code {expected_code}, got {:?}: {body}",
                error_code(&body)
            );
        }
    }
}

/// RULE: what each of the three actions answers is decided by the account's one stored
/// flag, and a refusal leaves that flag exactly as it was.
#[tokio::test]
async fn the_held_flag_decides_what_each_action_answers() {
    for row in state_rows() {
        let case = row.case;
        let (router, engine) = router_with_engine();
        row.held.reach(&router).await;
        let (status, body) = federation_call(&router, None, row.action).await;
        match &row.answer {
            Answer::Served => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the call to be served, got {status}: {body}"
                );
                assert!(
                    !body.contains("<Code>"),
                    "case {case:?}: expected a served response rather than an error document, \
                     got {body}"
                );
            }
            Answer::Refused {
                status: expected_status,
                code,
                message,
            } => {
                assert_eq!(
                    status, *expected_status,
                    "case {case:?}: expected {expected_status}, got {status}: {body}"
                );
                assert_eq!(
                    error_code(&body),
                    *code,
                    "case {case:?}: expected the code {code}, got {:?}: {body}",
                    error_code(&body)
                );
                assert_eq!(
                    &element(&body, "Message"),
                    message,
                    "case {case:?}: expected the message {message:?}, got {:?}",
                    element(&body, "Message")
                );
            }
        }
        assert_eq!(
            feature_is_on(&engine),
            row.on_after,
            "case {case:?}: expected the feature to be on={} after the call, got the other",
            row.on_after
        );
    }
}

/// RULE: repeating a call from one state answers that state's own answer every time — the
/// served call is what MOVES the flag, so the second call of a pair sees the state the
/// first one left.
#[tokio::test]
async fn a_repeated_call_answers_the_state_the_previous_one_left() {
    let (router, engine) = router_with_engine();

    // Two enables: the first turns the feature on, the second is refused because it is on.
    let (status, body) = federation_call(&router, None, ENABLE).await;
    assert_eq!(status, 200, "the first enable, got {status}: {body}");
    let first_issuer = element(&body, "IssuerIdentifier");
    let (status, body) = federation_call(&router, None, ENABLE).await;
    assert_eq!(
        status, FEATURE_ENABLED_STATUS,
        "the second enable, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        FEATURE_ENABLED_CODE,
        "the second enable, got {:?}: {body}",
        error_code(&body)
    );
    assert!(
        feature_is_on(&engine),
        "a refused second enable must leave the feature on"
    );

    // Two disables: the first turns it off, the second is refused because it is off.
    let (status, body) = federation_call(&router, None, DISABLE).await;
    assert_eq!(status, 200, "the first disable, got {status}: {body}");
    let (status, body) = federation_call(&router, None, DISABLE).await;
    assert_eq!(
        status, FEATURE_DISABLED_STATUS,
        "the second disable, got {status}: {body}"
    );
    assert_eq!(
        element(&body, "Message"),
        already_disabled_message(),
        "the second disable, got {:?}",
        element(&body, "Message")
    );
    assert!(
        !feature_is_on(&engine),
        "a refused second disable must leave the feature off"
    );

    // The issuer the first enable minted is the account's, so turning the feature on again
    // reports that same issuer rather than a second one.
    let (status, body) = federation_call(&router, None, ENABLE).await;
    assert_eq!(status, 200, "the enable after a disable, got {status}: {body}");
    assert_eq!(
        element(&body, "IssuerIdentifier"),
        first_issuer,
        "the account's issuer must survive a disable, got {:?} after {first_issuer:?}",
        element(&body, "IssuerIdentifier")
    );
}

/// RULE: the read reports the record the enable wrote — the account's own issuer and the
/// flag as it stands — and reporting it changes nothing a later caller sees.
#[tokio::test]
async fn the_read_reports_the_record_the_enable_wrote() {
    let (router, engine) = router_with_engine();
    let (status, body) = federation_call(&router, None, ENABLE).await;
    assert_eq!(status, 200, "the enable, got {status}: {body}");
    let minted = element(&body, "IssuerIdentifier");
    assert_eq!(
        held_issuer(&engine).as_deref(),
        Some(minted.as_str()),
        "the enable must record the issuer it answered, got {:?}",
        held_issuer(&engine)
    );

    // Two reads of one account answer the same members, and neither writes anything.
    for attempt in 0..2 {
        let case = format!("read attempt {attempt}");
        let (status, body) = federation_call(&router, None, INFO).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read to be served, got {status}: {body}"
        );
        assert_eq!(
            element(&body, "IssuerIdentifier"),
            minted,
            "case {case:?}: expected the issuer the enable minted, got {:?}",
            element(&body, "IssuerIdentifier")
        );
        assert_eq!(
            element(&body, "JwtVendingEnabled"),
            "true",
            "case {case:?}: expected the read to report the feature on, got {:?}",
            element(&body, "JwtVendingEnabled")
        );
        assert_eq!(
            elements(&body, "IssuerIdentifier").len(),
            1,
            "case {case:?}: expected exactly one <IssuerIdentifier>, got {body}"
        );
        assert_eq!(
            elements(&body, "JwtVendingEnabled").len(),
            1,
            "case {case:?}: expected exactly one <JwtVendingEnabled>, got {body}"
        );
        assert!(
            feature_is_on(&engine),
            "case {case:?}: a read must not change the account's state"
        );
    }
}

/// RULE: the issuer is a URL of the endpoint this emulator is served at, closed by one
/// opaque segment that is this account's own — so a client can fetch it back here, no two
/// accounts share one, and it does not depend on anything a caller sent.
#[tokio::test]
async fn the_issuer_is_an_address_of_this_emulator_derived_for_the_account() {
    let (router, _engine) = router_with_engine();
    let (status, body) = federation_call(&router, None, ENABLE).await;
    assert_eq!(status, 200, "the enable, got {status}: {body}");
    let issuer = element(&body, "IssuerIdentifier");

    let (base, segment) = issuer
        .rsplit_once('/')
        .unwrap_or_else(|| panic!("expected the issuer to carry a path segment, got {issuer:?}"));
    assert!(
        base.starts_with("https://"),
        "expected the issuer to be an https URL of this emulator, got {issuer:?}"
    );
    assert_eq!(
        segment.len(),
        ISSUER_SEGMENT_DIGITS,
        "expected the issuer's opaque segment to be {ISSUER_SEGMENT_DIGITS} digits long, \
         got {segment:?}"
    );
    assert!(
        segment
            .chars()
            .all(|digit| digit.is_ascii_hexdigit() && !digit.is_ascii_lowercase()),
        "expected the issuer's opaque segment to be upper-case hexadecimal, got {segment:?}"
    );

    // The derivation itself, over a table of accounts and served endpoints: one issuer per
    // account per endpoint, and no two accounts sharing one. The rows differ along both of
    // the two values the issuer is built from, including a trailing slash on the endpoint,
    // which must not double in the URL.
    let endpoints = ["https://emulator.invalid", "https://127.0.0.1:9297/"];
    // Three DISTINCT 12-digit accounts, this emulator's own among them, so the row about
    // two accounts not sharing an issuer is about two different accounts.
    let accounts = [account_id(), "123456789012", "210987654321"];
    assert_eq!(
        accounts[0].len(),
        12,
        "the emulator's account id is the 12-digit id every ARN carries, got {:?}",
        accounts[0]
    );
    for endpoint in endpoints {
        let mut seen: Vec<String> = Vec::new();
        for account in accounts {
            let case = format!("{endpoint} for {account}");
            let derived = state::outbound_web_identity_issuer(endpoint, account);
            assert_eq!(
                derived,
                state::outbound_web_identity_issuer(endpoint, account),
                "case {case:?}: the derivation must be stable, got {derived:?}"
            );
            assert!(
                derived.starts_with(endpoint.trim_end_matches('/')),
                "case {case:?}: expected the issuer under the served endpoint, got {derived:?}"
            );
            assert!(
                !derived.trim_start_matches("https://").contains("//"),
                "case {case:?}: expected one slash before the segment, got {derived:?}"
            );
            assert!(
                !seen.contains(&derived),
                "case {case:?}: expected an issuer no other account holds, got {derived:?} \
                 among {seen:?}"
            );
            seen.push(derived);
        }
    }
}

/// RULE: a served disable answers the empty result its unit output declares — the envelope
/// and nothing else — so a client reading a member finds none.
#[tokio::test]
async fn a_served_disable_answers_an_empty_result() {
    let (router, engine) = router_with_engine();
    let (status, body) = federation_call(&router, None, ENABLE).await;
    assert_eq!(status, 200, "the fixture enable, got {status}: {body}");
    let (status, body) = federation_call(&router, None, DISABLE).await;
    assert_eq!(status, 200, "the disable, got {status}: {body}");
    assert!(
        body.contains("DisableOutboundWebIdentityFederationResponse"),
        "expected the operation's own response envelope, got {body}"
    );
    assert!(
        elements(&body, "IssuerIdentifier").is_empty()
            && elements(&body, "JwtVendingEnabled").is_empty(),
        "expected an empty result, got {body}"
    );
    assert!(
        !feature_is_on(&engine),
        "a served disable must turn the feature off"
    );
    // The issuer stays on the record: the disable "does not affect tokens that were issued
    // before the feature was disabled", and the issuer belongs to the account.
    assert!(
        held_issuer(&engine).is_some(),
        "a disable must leave the account's issuer on the record"
    );
}

/// RULE: IAM decides each of the three calls against the caller's identity-based policies
/// before it reports or writes anything — the feature is the ACCOUNT's, so the decision is
/// made on the account-wide resource — and a refused call leaves the record alone.
#[tokio::test]
async fn a_caller_whose_policies_do_not_allow_the_call_is_refused() {
    // Rows: each of the three actions, each refused twice — once with no allow at all and
    // once with an explicit deny over a grant — and then served under a grant that names
    // its action, so the gate is shown deciding rather than refusing everything.
    let actions = [
        (ENABLE, "iam:EnableOutboundWebIdentityFederation", Held::NeverEnabled),
        (INFO, "iam:GetOutboundWebIdentityFederationInfo", Held::Enabled),
        (DISABLE, "iam:DisableOutboundWebIdentityFederation", Held::Enabled),
    ];
    for (row, (action, iam_action, held)) in actions.into_iter().enumerate() {
        let refusals = vec![
            (
                "no allow at all",
                vec![(
                    "unrelated".to_string(),
                    statement("Allow", "iam:ListUsers", "*"),
                )],
                format!("because no identity-based policy allows the {iam_action} action"),
            ),
            (
                "an explicit deny over the allow",
                vec![
                    ("allow".to_string(), statement("Allow", iam_action, "*")),
                    ("deny".to_string(), statement("Deny", iam_action, "*")),
                ],
                "with an explicit deny in an identity-based policy".to_string(),
            ),
        ];
        for (attempt, (refusal, policies, reason)) in refusals.into_iter().enumerate() {
            let case = format!("{action} with {refusal}");
            let (router, engine) = router_with_engine();
            held.reach(&router).await;
            let was_on = feature_is_on(&engine);
            let user_name = format!("federator{row}{attempt}");
            let access_key_id = principal(&router, &case, &user_name, &policies).await;
            let (status, body) = federation_call(&router, Some(&access_key_id), action).await;
            assert_eq!(
                status, 403,
                "case {case:?}: expected 403, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "AccessDenied",
                "case {case:?}: expected AccessDenied, got {:?}: {body}",
                error_code(&body)
            );
            assert_access_denied_message(
                &case,
                &element(&body, "Message"),
                &format!("arn:aws:iam::{}:user/{user_name}", account_id()),
                iam_action,
                state::authorization::ACCOUNT_WIDE_RESOURCE,
                &reason,
            );
            assert_eq!(
                feature_is_on(&engine),
                was_on,
                "case {case:?}: a refused call must leave the account's state alone"
            );
        }

        let case = format!("{action} under a grant naming it");
        let (router, _engine) = router_with_engine();
        held.reach(&router).await;
        let access_key_id = principal(
            &router,
            &case,
            &format!("federator-granted{row}"),
            &[("grant".to_string(), statement("Allow", iam_action, "*"))],
        )
        .await;
        let (status, body) = federation_call(&router, Some(&access_key_id), action).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the granted call to be served, got {status}: {body}"
        );
    }
}
