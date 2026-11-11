// SPDX-License-Identifier: CC0-1.0

//! `DeleteConnector` is asynchronous: the teardown is observable before the record is gone.
//!
//! The rule under test: an operation whose model answers HTTP 202 with an empty body only
//! ACCEPTS the work, so `DeleteConnector` accepts the call whatever non-terminal status the
//! connector is in, moves it into the modelled `ConnectorStatus` value `DELETING` — which
//! every reader reports for the teardown window — and drops the record, with the challenges
//! it owns, only once the clock makes the teardown due.
//!
//! Every case drives the service the way a client does, one HTTP request per call through the
//! same `MultiServiceRouter` the emulator boots. The table's rows differ along what the rule
//! generalises over: the status the connector is in when the delete arrives, its
//! `ConnectorType`, how many challenges it owns and which CA it was created from — the status
//! the issue was filed about (`ACTIVE`, no challenges) is ONE row.

use std::time::SystemTime;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{self, label, new_router, Answer};
use service_com_amazonaws_pcaconnectorscep_pcaconnectorscep::state;

/// A region that is NOT the emulator default, so an ARN built from the request's own SigV4
/// credential scope is distinguishable from a hardcoded one.
const REGION: &str = "eu-west-2";

/// The `ConnectorStatus` values this suite moves a connector through, named from the
/// service's own constants rather than respelled here.
const STATUS_CREATING: &str = state::STATUS_CREATING;
/// The settled status the clock owes a new connector.
const STATUS_ACTIVE: &str = state::STATUS_ACTIVE;
/// The status a connector carries while its teardown runs.
const STATUS_DELETING: &str = state::STATUS_DELETING;

/// Private-CA ARNs matching `CertificateAuthorityArn`'s own pattern
/// (`^arn:aws(-[a-z]+)*:acm-pca:<region>:<account>:certificate-authority/<uuid>$`). Rows name
/// different CAs, so no assertion can pass by echoing one recorded value.
const CA_ARNS: &[&str] = &[
    "arn:aws:acm-pca:eu-west-2:000000000000:certificate-authority/11111111-2222-3333-4444-555555555555",
    "arn:aws:acm-pca:eu-west-2:000000000000:certificate-authority/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
    "arn:aws:acm-pca:eu-west-2:000000000000:certificate-authority/0123456f-89ab-cdef-0123-456789abcdef",
    "arn:aws:acm-pca:eu-west-2:000000000000:certificate-authority/deadbeef-0000-1111-2222-333344445555",
    "arn:aws:acm-pca:eu-west-2:000000000000:certificate-authority/99999999-8888-7777-6666-555544443333",
    "arn:aws:acm-pca:eu-west-2:000000000000:certificate-authority/abcdef01-2345-6789-abcd-ef0123456789",
];

/// The status a connector is in when the delete arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartingStatus {
    /// The clock still owes the connector its `ACTIVE`.
    Creating,
    /// The connector is settled and serving.
    Active,
}

/// Whether the connector was created with an `IntuneConfiguration`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    GeneralPurpose,
    Intune,
}

/// One row of the table: a connector to build, and the state the delete has to cope with.
#[derive(Debug)]
struct Case {
    name: &'static str,
    ca_arn: &'static str,
    kind: Kind,
    start: StartingStatus,
    /// How many challenge passwords the connector owns when the delete arrives. Only an
    /// `ACTIVE` general-purpose connector can mint one, so the other rows carry none.
    challenges: usize,
}

async fn create_connector(router: &MultiServiceRouter, request: Value) -> Answer {
    harness::call(router, REGION, "POST", "/connectors", Some(request)).await
}

async fn get_connector(router: &MultiServiceRouter, arn: &str) -> Answer {
    harness::call(
        router,
        REGION,
        "GET",
        &format!("/connectors/{}", label(arn)),
        None,
    )
    .await
}

async fn list_connectors(router: &MultiServiceRouter) -> Answer {
    harness::call(router, REGION, "GET", "/connectors", None).await
}

async fn delete_connector(router: &MultiServiceRouter, arn: &str) -> Answer {
    harness::call(
        router,
        REGION,
        "DELETE",
        &format!("/connectors/{}", label(arn)),
        None,
    )
    .await
}

async fn create_challenge(router: &MultiServiceRouter, request: Value) -> Answer {
    harness::call(router, REGION, "POST", "/challenges", Some(request)).await
}

async fn get_challenge_password(router: &MultiServiceRouter, arn: &str) -> Answer {
    harness::call(
        router,
        REGION,
        "GET",
        &format!("/challengePasswords/{}", label(arn)),
        None,
    )
    .await
}

/// The `Status` a listing reports for `arn`, or `None` when the listing does not carry it.
fn listed_status(answer: &Answer, arn: &str) -> Option<String> {
    answer.body["Connectors"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a `Connectors` array in {:?}", answer.body))
        .iter()
        .find(|entry| entry["Arn"].as_str() == Some(arn))
        .map(|entry| {
            entry["Status"]
                .as_str()
                .unwrap_or_else(|| panic!("expected a `Status` in {entry:?}"))
                .to_string()
        })
}

/// The instant the clock owes `arn` its pending transition, read through the service's own
/// state; `None` once the connector is settled or gone.
fn pending_due_at(engine: &engine::InMemoryEngine, arn: &str) -> Option<SystemTime> {
    let shared = state::service_state(engine).expect("the service's own state");
    let store = shared.lock();
    let connector = store.connectors.get(arn)?;
    connector.pending.as_ref().map(|pending| pending.due_at)
}

/// Build the connector a row describes and bring it into the row's starting status, answering
/// its ARN and the ARNs of the challenges it owns.
async fn arrange(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    case: &Case,
) -> (String, Vec<String>) {
    let mut request = json!({ "CertificateAuthorityArn": case.ca_arn });
    if case.kind == Kind::Intune {
        request["MobileDeviceManagement"] = json!({
            "Intune": {
                "AzureApplicationId": "01234567-89ab-cdef-0123-456789abcdef",
                "Domain": "example.onmicrosoft.com",
            }
        });
    }
    let created = create_connector(router, request).await;
    assert_eq!(
        created.status, 202,
        "case {:?}: expected 202 from CreateConnector, got {} / {:?}",
        case.name, created.status, created.body
    );
    let arn = created.body["ConnectorArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {:?}: expected a ConnectorArn in {:?}", case.name, created.body))
        .to_string();

    let expected_status = match case.start {
        // Hold the `ACTIVE` the clock owes it, so the CREATING branch is the one every run
        // exercises rather than one the test had to win a race for.
        StartingStatus::Creating => {
            harness::hold_pending(engine, &arn);
            STATUS_CREATING
        }
        StartingStatus::Active => {
            harness::release_pending(engine, &arn);
            STATUS_ACTIVE
        }
    };
    let reported = get_connector(router, &arn).await;
    assert_eq!(
        reported.body["Connector"]["Status"],
        json!(expected_status),
        "case {:?}: the connector must report {expected_status} before the delete, got {:?}",
        case.name,
        reported.body
    );

    let mut challenges = Vec::new();
    for index in 0..case.challenges {
        let answer = create_challenge(router, json!({ "ConnectorArn": arn.clone() })).await;
        assert_eq!(
            answer.status, 202,
            "case {:?}: expected 202 from CreateChallenge #{index}, got {} / {:?}",
            case.name, answer.status, answer.body
        );
        challenges.push(
            answer.body["Challenge"]["Arn"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {:?}: expected a challenge Arn in {:?}", case.name, answer.body)
                })
                .to_string(),
        );
    }
    (arn, challenges)
}

/// The whole rule, row by row: 202, then `DELETING` on every reader, then — and only then —
/// the record and its challenges are gone.
#[tokio::test]
async fn delete_connector_is_accepted_and_the_teardown_is_observable() {
    let cases = [
        Case {
            name: "active general-purpose, no challenges",
            ca_arn: CA_ARNS[0],
            kind: Kind::GeneralPurpose,
            start: StartingStatus::Active,
            challenges: 0,
        },
        Case {
            name: "active general-purpose, one challenge",
            ca_arn: CA_ARNS[1],
            kind: Kind::GeneralPurpose,
            start: StartingStatus::Active,
            challenges: 1,
        },
        Case {
            name: "active general-purpose, three challenges",
            ca_arn: CA_ARNS[2],
            kind: Kind::GeneralPurpose,
            start: StartingStatus::Active,
            challenges: 3,
        },
        Case {
            name: "still creating, general-purpose",
            ca_arn: CA_ARNS[3],
            kind: Kind::GeneralPurpose,
            start: StartingStatus::Creating,
            challenges: 0,
        },
        Case {
            name: "active intune",
            ca_arn: CA_ARNS[4],
            kind: Kind::Intune,
            start: StartingStatus::Active,
            challenges: 0,
        },
        Case {
            name: "still creating, intune",
            ca_arn: CA_ARNS[5],
            kind: Kind::Intune,
            start: StartingStatus::Creating,
            challenges: 0,
        },
    ];

    for case in &cases {
        let (router, engine) = new_router();
        let (arn, challenges) = arrange(&router, &engine, case).await;
        let case_name = case.name;

        // The call only accepts the work — whatever non-terminal status the connector is in.
        let deleted = delete_connector(&router, &arn).await;
        assert_eq!(
            (deleted.status, deleted.error_type.as_str()),
            (202, ""),
            "case {case_name:?}: DeleteConnector must accept the work with 202, got {} / {:?}",
            deleted.status,
            deleted.body
        );

        // Straight after the delete — no clock help — the connector is still reported, in
        // DELETING: the teardown window is never zero.
        let reported = get_connector(&router, &arn).await;
        assert_eq!(
            reported.status, 200,
            "case {case_name:?}: GetConnector right after the delete must still answer 200, got \
             {} / {:?}",
            reported.status, reported.body
        );
        assert_eq!(
            reported.body["Connector"]["Status"],
            json!(STATUS_DELETING),
            "case {case_name:?}: expected Status {STATUS_DELETING}, got {:?}",
            reported.body
        );
        assert_eq!(
            reported.body["Connector"]["Arn"],
            json!(arn),
            "case {case_name:?}: the connector under teardown must keep its own Arn, got {:?}",
            reported.body
        );
        assert_eq!(
            reported.body["Connector"]["StatusReason"],
            Value::Null,
            "case {case_name:?}: StatusReason is for a FAILED connector only, got {:?}",
            reported.body
        );

        // From here the case is about the window, so pin it rather than race TRANSITION_SPAN.
        harness::hold_pending(&engine, &arn);

        // The listing agrees with the get: one state, two readers.
        let listed = list_connectors(&router).await;
        assert_eq!(
            listed_status(&listed, &arn).as_deref(),
            Some(STATUS_DELETING),
            "case {case_name:?}: ListConnectors must report {STATUS_DELETING} for {arn:?}, got {:?}",
            listed.body
        );

        // A connector being torn down is not serving, so nothing new can be built on it.
        if case.kind == Kind::GeneralPurpose {
            let refused = create_challenge(&router, json!({ "ConnectorArn": arn.clone() })).await;
            assert_eq!(
                (refused.status, refused.error_type.as_str()),
                (400, "ValidationException"),
                "case {case_name:?}: CreateChallenge on a {STATUS_DELETING} connector must be \
                 refused, got {} / {:?}",
                refused.status,
                refused.body
            );
            assert_eq!(
                refused.body["Reason"],
                json!("INVALID_STATE"),
                "case {case_name:?}: expected Reason INVALID_STATE, got {:?}",
                refused.body
            );
        }

        // `@idempotent`: a repeat is accepted, reports the same status, and can neither
        // extend nor restart the teardown the first call scheduled.
        let due_before = pending_due_at(&engine, &arn);
        let again = delete_connector(&router, &arn).await;
        assert_eq!(
            again.status, 202,
            "case {case_name:?}: a repeated DeleteConnector must still answer 202, got {} / {:?}",
            again.status, again.body
        );
        assert_eq!(
            pending_due_at(&engine, &arn),
            due_before,
            "case {case_name:?}: a repeated delete must not move the teardown's due instant"
        );
        let reported = get_connector(&router, &arn).await;
        assert_eq!(
            reported.body["Connector"]["Status"],
            json!(STATUS_DELETING),
            "case {case_name:?}: the repeat must leave the status at {STATUS_DELETING}, got {:?}",
            reported.body
        );

        // Once the clock makes the teardown due the record is gone, challenges with it.
        harness::release_pending(&engine, &arn);
        let gone = get_connector(&router, &arn).await;
        assert_eq!(
            (gone.status, gone.error_type.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case_name:?}: once the teardown is due the connector must answer 404, got {} / \
             {:?}",
            gone.status,
            gone.body
        );
        let listed = list_connectors(&router).await;
        assert_eq!(
            listed_status(&listed, &arn),
            None,
            "case {case_name:?}: a torn-down connector must not be listed, got {:?}",
            listed.body
        );
        for challenge in &challenges {
            let read = get_challenge_password(&router, challenge).await;
            assert_eq!(
                (read.status, read.error_type.as_str()),
                (404, "ResourceNotFoundException"),
                "case {case_name:?}: the teardown must take challenge {challenge:?} with it, got \
                 {} / {:?}",
                read.status,
                read.body
            );
        }

        // Once the record is gone the ARN names nothing, so the delete has nothing left to
        // schedule: it is the same accepted 202 — and resurrects nothing.
        let after = delete_connector(&router, &arn).await;
        assert_eq!(
            (after.status, after.error_type.as_str()),
            (202, ""),
            "case {case_name:?}: a delete of a gone connector must answer 202, got {} / {:?}",
            after.status,
            after.body
        );
        let still_gone = get_connector(&router, &arn).await;
        assert_eq!(
            still_gone.status, 404,
            "case {case_name:?}: the accepted delete must not recreate the connector, got {} / {:?}",
            still_gone.status, still_gone.body
        );
    }
}

/// The delete does not wait for a pending `CREATING` to settle: it replaces that transition
/// with the teardown, so the connector never reappears as `ACTIVE` afterwards.
#[tokio::test]
async fn deleting_a_creating_connector_replaces_the_pending_active() {
    for ca_arn in [CA_ARNS[0], CA_ARNS[3]] {
        let (router, engine) = new_router();
        let created = create_connector(&router, json!({ "CertificateAuthorityArn": ca_arn })).await;
        let arn = created.body["ConnectorArn"]
            .as_str()
            .unwrap_or_else(|| panic!("expected a ConnectorArn in {:?}", created.body))
            .to_string();
        harness::hold_pending(&engine, &arn);

        let deleted = delete_connector(&router, &arn).await;
        assert_eq!(
            deleted.status, 202,
            "case {ca_arn:?}: a delete while {STATUS_CREATING} must be accepted, got {} / {:?}",
            deleted.status, deleted.body
        );

        // Hold the teardown and let the clock run: the only transition left is the teardown,
        // so no reader can ever see this connector settle into ACTIVE.
        harness::hold_pending(&engine, &arn);
        let reported = get_connector(&router, &arn).await;
        assert_eq!(
            reported.body["Connector"]["Status"],
            json!(STATUS_DELETING),
            "case {ca_arn:?}: the pending {STATUS_ACTIVE} must be replaced by the teardown, got \
             {:?}",
            reported.body
        );

        harness::release_pending(&engine, &arn);
        let gone = get_connector(&router, &arn).await;
        assert_eq!(
            (gone.status, gone.error_type.as_str()),
            (404, "ResourceNotFoundException"),
            "case {ca_arn:?}: the teardown must complete, got {} / {:?}",
            gone.status,
            gone.body
        );
    }
}

/// A `ClientToken` answers for the connector it created only while that connector is still
/// there: once the delete is accepted the token's connector is tearing down, so a retry
/// creates a new one rather than handing back a connector that is about to disappear.
#[tokio::test]
async fn a_client_token_does_not_answer_with_a_connector_under_teardown() {
    for (case, token) in [
        ("uuid token", "11111111-2222-3333-4444-555555555555"),
        ("opaque token", "retry-token-one"),
    ] {
        let (router, engine) = new_router();
        let request = json!({ "CertificateAuthorityArn": CA_ARNS[2], "ClientToken": token });
        let created = create_connector(&router, request.clone()).await;
        let first = created.body["ConnectorArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a ConnectorArn in {:?}", created.body))
            .to_string();
        harness::release_pending(&engine, &first);

        // The same token, before the delete: the same connector, not a second one.
        let retried = create_connector(&router, request.clone()).await;
        assert_eq!(
            retried.body["ConnectorArn"],
            json!(first),
            "case {case:?}: the token must answer with the connector it created, got {:?}",
            retried.body
        );

        assert_eq!(
            delete_connector(&router, &first).await.status,
            202,
            "case {case:?}: expected 202 from DeleteConnector"
        );
        harness::hold_pending(&engine, &first);

        let after = create_connector(&router, request).await;
        let second = after.body["ConnectorArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a ConnectorArn in {:?}", after.body))
            .to_string();
        assert_ne!(
            second, first,
            "case {case:?}: a token whose connector is tearing down must mint a new connector, \
             got {second:?}"
        );
        let reported = get_connector(&router, &second).await;
        assert_eq!(
            reported.body["Connector"]["Status"],
            json!(STATUS_CREATING),
            "case {case:?}: the new connector must start {STATUS_CREATING}, got {:?}",
            reported.body
        );
    }
}
