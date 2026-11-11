// SPDX-License-Identifier: CC0-1.0

//! `GetSessionEndpoint`, end to end over HTTP.
//!
//! Every assertion goes through the real router (restJson1, SigV4 signing name
//! `emr-serverless`), so each case exercises routing, deserialization, the handler, the
//! persisted session and serialization — the same path an SDK client takes.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one
//! request:
//!
//! 1. The operation reports the STORED session: the application id and session id the
//!    record carries, an `endpoint` satisfying `EndpointUrl` that is the same for every
//!    call about one session and different between two sessions, and an `authToken`
//!    satisfying `SessionAuthToken` whose `authTokenExpiresAt` is still ahead of the
//!    call.
//! 2. "Call this operation again when the authentication token expires to obtain a new
//!    token": every call mints a token of its own, while the endpoint that token is for
//!    stays put.
//! 3. A client can connect to a session that has not ended: every non-terminal
//!    `SessionState` is served, and each terminal one is refused with a 400
//!    `ValidationException`.
//! 4. Nothing is minted for a session that does not resolve: whether the application
//!    holds no such session or does not exist at all, the request is refused as input
//!    validation — HTTP 400 `ValidationException` naming the `sessionId` member.
//! 5. The call is a report — two reads of an unchanged session agree across it.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_emrserverless_awstoledowebservice::state::ComAmazonawsEmrserverlessAwstoledowebserviceState;

use super::application_lifecycle::{call, emulator, emulator_with_engine, wait_for_transition};
use super::session_lifecycle::{get_session, minimal_session, session_application, start_session};

/// `SessionAuthToken`'s modelled `@length(min: 1, max: 8000)`
/// (`smithy_query(query="shape",shape="SessionAuthToken")`).
const AUTH_TOKEN_LENGTH: std::ops::RangeInclusive<usize> = 1..=8000;

/// `EndpointUrl`'s modelled `@length(min: 1, max: 256)`
/// (`smithy_query(query="shape",shape="EndpointUrl")`).
const ENDPOINT_LENGTH: std::ops::RangeInclusive<usize> = 1..=256;

/// Every value `SessionState` admits, in the order the model lists them
/// (`smithy_query(query="shape",shape="SessionState")`), paired with whether a session
/// in it has ended: the two terminal values are the ones a client can no longer connect
/// to.
const SESSION_STATES: &[(&str, bool)] = &[
    ("SUBMITTED", false),
    ("STARTING", false),
    ("STARTED", false),
    ("IDLE", false),
    ("BUSY", false),
    ("FAILED", true),
    ("TERMINATING", false),
    ("TERMINATED", true),
];

async fn get_session_endpoint(
    router: &MultiServiceRouter,
    application_id: &str,
    session_id: &str,
) -> super::application_lifecycle::Answer {
    call(
        router,
        "GET",
        &format!("/applications/{application_id}/sessions/{session_id}/endpoint"),
        None,
    )
    .await
}

/// Start a session on `application_id` and answer its id.
async fn started_session(
    router: &MultiServiceRouter,
    application_id: &str,
    body: Value,
) -> String {
    let answer = start_session(router, application_id, body).await;
    assert_eq!(
        answer.status, 200,
        "StartSession expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer
        .at("/sessionId")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Epoch seconds, as the wire renders a modelled timestamp.
fn now_epoch_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .expect("the test clock is after the epoch")
        .as_secs_f64()
}

/// One row of the endpoint table: the session the call is made about.
struct EndpointCase {
    case: &'static str,
    body: Value,
    /// Whether the call is made after the clock owed the session its `STARTED` state,
    /// rather than straight after the start returned.
    settled: bool,
    /// Whether the session is started on an application of its own.
    own_application: bool,
}

fn endpoint_cases() -> Vec<EndpointCase> {
    vec![
        EndpointCase {
            case: "a session read straight after the start, in its transient state",
            body: minimal_session("tok-endpoint-transient"),
            settled: false,
            own_application: false,
        },
        EndpointCase {
            case: "a session read once the clock has brought it up",
            body: minimal_session("tok-endpoint-started"),
            settled: true,
            own_application: false,
        },
        EndpointCase {
            case: "a named session carrying tags and runtime overrides",
            body: {
                let mut body = minimal_session("tok-endpoint-named");
                body["name"] = json!("analytics");
                body["tags"] = json!({ "team": "data", "env": "test" });
                body["configurationOverrides"] = json!({
                    "runtimeConfiguration": [{
                        "classification": "spark-defaults",
                        "properties": { "spark.executor.cores": "2" },
                    }],
                });
                body
            },
            settled: true,
            own_application: false,
        },
        EndpointCase {
            case: "a session with an idle timeout, on an application of its own",
            body: {
                let mut body = minimal_session("tok-endpoint-idle");
                body["idleTimeoutMinutes"] = json!(1_000_000);
                body
            },
            settled: false,
            own_application: true,
        },
    ]
}

/// Rules 1 and 2, over the table above: the reported endpoint names the stored session
/// and repeats, the token is fresh on every call and expires ahead of the call, and two
/// sessions never share an endpoint.
#[tokio::test]
async fn get_session_endpoint_reports_the_stored_session_and_a_fresh_token() {
    let router = emulator();
    let (shared_application, _) = session_application(&router, "tok-endpoint-app").await;

    let mut endpoints: Vec<(String, String)> = Vec::new();
    for (index, case) in endpoint_cases().into_iter().enumerate() {
        let label = case.case;
        let application_id = if case.own_application {
            session_application(&router, &format!("tok-endpoint-app-{index}"))
                .await
                .0
        } else {
            shared_application.clone()
        };
        let session_id = started_session(&router, &application_id, case.body).await;
        if case.settled {
            wait_for_transition();
        }

        let before = now_epoch_seconds();
        let answer = get_session_endpoint(&router, &application_id, &session_id).await;
        assert_eq!(
            answer.status, 200,
            "case {label:?}: GetSessionEndpoint expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.at("/applicationId").as_str(),
            Some(application_id.as_str()),
            "case {label:?}: expected applicationId {application_id:?}, got {:?}",
            answer.at("/applicationId")
        );
        assert_eq!(
            answer.at("/sessionId").as_str(),
            Some(session_id.as_str()),
            "case {label:?}: expected sessionId {session_id:?}, got {:?}",
            answer.at("/sessionId")
        );

        let endpoint = answer
            .at("/endpoint")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            ENDPOINT_LENGTH.contains(&endpoint.chars().count()),
            "case {label:?}: endpoint must satisfy EndpointUrl's length, got {endpoint:?}"
        );
        assert!(
            endpoint.starts_with("https://"),
            "case {label:?}: endpoint must be a URL a client can connect to, got {endpoint:?}"
        );
        assert!(
            endpoint.contains(&session_id),
            "case {label:?}: endpoint must name the session it is for, got {endpoint:?}"
        );

        let token = answer
            .at("/authToken")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            AUTH_TOKEN_LENGTH.contains(&token.chars().count()),
            "case {label:?}: authToken must satisfy SessionAuthToken's length, got {} \
             characters",
            token.chars().count()
        );
        assert!(
            !token.contains(&session_id) && !token.contains(&application_id),
            "case {label:?}: the token is a minted secret, not the request's own values, \
             got {token:?}"
        );
        let expires_at = answer
            .at("/authTokenExpiresAt")
            .as_f64()
            .unwrap_or_default();
        assert!(
            expires_at > before,
            "case {label:?}: authTokenExpiresAt must be ahead of the call, got \
             {expires_at} against {before}"
        );

        // Rule 2: the next call hands out a token of its own for the same endpoint.
        let again = get_session_endpoint(&router, &application_id, &session_id).await;
        assert_eq!(
            again.status, 200,
            "case {label:?}: the second GetSessionEndpoint expected 200, got {} with {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.at("/endpoint").as_str(),
            Some(endpoint.as_str()),
            "case {label:?}: one session has one endpoint, got {:?} then {:?}",
            endpoint,
            again.at("/endpoint")
        );
        assert_ne!(
            again.at("/authToken").as_str(),
            Some(token.as_str()),
            "case {label:?}: calling again must obtain a new token, got the same one twice"
        );
        assert!(
            again
                .at("/authTokenExpiresAt")
                .as_f64()
                .unwrap_or_default()
                >= expires_at,
            "case {label:?}: a later token expires no earlier than the one before it, got \
             {:?} against {expires_at}",
            again.at("/authTokenExpiresAt")
        );

        endpoints.push((session_id, endpoint));
    }

    for (index, (session_id, endpoint)) in endpoints.iter().enumerate() {
        for (other_id, other_endpoint) in endpoints.iter().skip(index + 1) {
            assert_ne!(
                endpoint, other_endpoint,
                "case {session_id:?} against {other_id:?}: two sessions never share an \
                 endpoint, got {endpoint:?} twice"
            );
        }
    }
}

/// Rule 3, over every value `SessionState` admits: a session that has not ended is
/// served, and one that has is refused with the modelled `ValidationException`.
///
/// The state is written through the store the handlers share, because the states a
/// session reaches through its own lifecycle are only some of the catalog, and the rule
/// holds over all of it.
#[tokio::test]
async fn get_session_endpoint_serves_a_session_that_has_not_ended() {
    let (router, engine) = emulator_with_engine();
    let (application_id, _) = session_application(&router, "tok-endpoint-states").await;

    for (index, (state, ended)) in SESSION_STATES.iter().enumerate() {
        let session_id = started_session(
            &router,
            &application_id,
            minimal_session(&format!("tok-endpoint-state-{index}")),
        )
        .await;
        force_session_state(&engine, &application_id, &session_id, state);

        let answer = get_session_endpoint(&router, &application_id, &session_id).await;
        let (expected_status, expected_code) = if *ended {
            (400, "ValidationException")
        } else {
            (200, "")
        };
        assert_eq!(
            answer.status, expected_status,
            "case {state:?}: expected {expected_status}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            expected_code,
            "case {state:?}: expected error code {expected_code:?}, got {:?} with {:?}",
            answer.error_code(),
            answer.body
        );
        if !*ended {
            assert!(
                answer
                    .at("/authToken")
                    .as_str()
                    .is_some_and(|token| AUTH_TOKEN_LENGTH.contains(&token.chars().count())),
                "case {state:?}: a live session is handed a token, got {:?}",
                answer.at("/authToken")
            );
        }
    }
}

/// Rule 4: nothing is minted for a session that does not resolve — asserted over the
/// ways a session reference can fail to name one, since the rule generalises over all of
/// them: the application is unknown, the application is unknown but the session id is
/// one that exists elsewhere, the session id is unknown under a known application, and
/// the session id is well-formed but was never minted.
#[tokio::test]
async fn get_session_endpoint_refuses_what_does_not_exist() {
    /// The sentence the refusal carries. Written out here rather than read from
    /// `crate::state`, so a wrong sentence in `src/` cannot make this test pass: fixed
    /// text naming the input member, with no id of the request inside it.
    const SESSION_ID_INVALID: &str = "SessionId is invalid.";

    let router = emulator();
    let (application_id, _) = session_application(&router, "tok-endpoint-missing").await;
    let session_id = started_session(
        &router,
        &application_id,
        minimal_session("tok-endpoint-present"),
    )
    .await;

    let cases: Vec<(&str, &str, &str)> = vec![
        ("an unknown application", "nosuchapplication", "nosuchsession"),
        (
            "an unknown application holding a known session id",
            "nosuchapplication",
            session_id.as_str(),
        ),
        ("an unknown session", application_id.as_str(), "nosuchsession"),
        (
            "a session id of the right shape that was never minted",
            application_id.as_str(),
            "00000000deadbeef",
        ),
    ];
    for (case, application, session) in cases {
        let answer = get_session_endpoint(&router, application, session).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?} with {:?}",
            answer.error_code(),
            answer.body
        );
        assert_eq!(
            answer.resolved_error_shape(),
            Some("ValidationException"),
            "case {case:?}: expected x-amzn-errortype to resolve to ValidationException, got {:?}",
            answer.resolved_error_shape()
        );
        assert_eq!(
            answer.at("/message").as_str().unwrap_or_default(),
            SESSION_ID_INVALID,
            "case {case:?}: expected the fixed sentence {SESSION_ID_INVALID:?}, got {:?}",
            answer.body
        );
        assert!(
            !answer.body.to_string().contains(session),
            "case {case:?}: the submitted session id must not be echoed, got {:?}",
            answer.body
        );
    }
}

/// Rule 5: the call reports, so what a later reader sees is unchanged by it — asserted
/// over sessions differing in whether the clock had already brought them up.
#[tokio::test]
async fn get_session_endpoint_changes_nothing_a_later_reader_sees() {
    let router = emulator();
    let (application_id, _) = session_application(&router, "tok-endpoint-readonly").await;

    for (case, settle) in [("a session still coming up", false), ("a started session", true)] {
        let session_id = started_session(
            &router,
            &application_id,
            minimal_session(&format!("tok-endpoint-ro-{}", u8::from(settle))),
        )
        .await;
        if settle {
            wait_for_transition();
        }

        let before = get_session(&router, &application_id, &session_id).await;
        let endpoint = get_session_endpoint(&router, &application_id, &session_id).await;
        assert_eq!(
            endpoint.status, 200,
            "case {case:?}: GetSessionEndpoint expected 200, got {} with {:?}",
            endpoint.status, endpoint.body
        );
        let after = get_session(&router, &application_id, &session_id).await;
        assert_eq!(
            after.body, before.body,
            "case {case:?}: the endpoint call must leave the session as it found it, got \
             {:?} then {:?}",
            before.body, after.body
        );
        // Nothing of the minted secret reaches the session a later caller reads.
        let token = endpoint.at("/authToken").as_str().unwrap_or_default().to_string();
        assert!(
            !after.body.to_string().contains(&token),
            "case {case:?}: the minted token must not be persisted on the session, got {:?}",
            after.body
        );
    }
}

/// Put `state` on the stored session, through the store the handlers share, and drop
/// whatever transition the clock still owed it: the rule under test is about the state
/// the session has settled on.
fn force_session_state(
    engine: &engine::InMemoryEngine,
    application_id: &str,
    session_id: &str,
    state: &str,
) {
    let store = engine.state::<ComAmazonawsEmrserverlessAwstoledowebserviceState>();
    let mut store = store.lock();
    let record = store
        .applications
        .get_mut(application_id)
        .expect("the application the test created is in the store");
    let session = record
        .sessions
        .get_mut(session_id)
        .expect("the session the test started is in the store");
    session.session.state = state.to_string();
    session.pending = None;
}
