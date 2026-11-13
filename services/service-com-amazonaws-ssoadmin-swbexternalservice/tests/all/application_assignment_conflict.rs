// SPDX-License-Identifier: CC0-1.0

//! The DUPLICATE GRANT of a direct application assignment, end to end over the
//! `awsJson1_1` wire.
//!
//! THE RULE under test: a direct application assignment is identified by the tuple
//! (application, principal type, principal id), so the grant CONVERGES on that one
//! assignment — a request for a tuple the application already holds answers with the same
//! HTTP 200 and the same modelled EMPTY body the first grant answered with, stores no
//! second row, and leaves the assignment readable; the application holds exactly one
//! assignment per tuple however often, and however concurrently, the grant is sent. The
//! duplicate is therefore NOT a refusal: the modelled `ConflictException` is for "a
//! conflict with a previous successful write … A retry (with appropriate backoff logic) is
//! the recommended response", a write this host has not seen yet — and converging is
//! exactly what makes that recommended retry work. The empty output and the error set come
//! from `smithy_query(query="operation", operation="CreateApplicationAssignment")`.
//!
//! The rule is asserted over a TABLE whose rows differ along what it generalises over: the
//! `PrincipalType` (`USER | GROUP`), the FORM the model's `PrincipalId` pattern admits (a
//! bare GUID in lower and in upper hex, and the `[0-9a-f]{10}-` prefixed form at the
//! 47-character length limit), the APPLICATION the grant is made on, the NUMBER of repeats
//! and whether the repeats are sent one after another or all at once.
//! `application_assignment_repeat_grant.rs` is the PROPERTY form of the same invariant,
//! over drawn populations of tuples.
//!
//! What converging does NOT swallow is asserted too: an `ApplicationArn` no create minted
//! is still refused `ResourceNotFoundException`, and a grant made after a delete removed
//! the assignment creates it afresh.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes, so the modelled constraints and the `awsJson1_1` envelope are exercised
//! rather than bypassed. The harness is the one `ssoadmin_permission_sets.rs` owns.
//!
//! Every modelled 4xx refusal of this service is answered with HTTP 400 whatever
//! `@httpError` its shape declares, so the refusal assertion below pins the status AND the
//! wire error code, which is what tells one refusal from another.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::application_assignments::{minted_application, Application};
use crate::ssoadmin_permission_sets::{
    assert_refused, call, create_application_assignment as grant, describe_application_assignment,
    router, router_over, seeded_instance_arn, with_store,
};

/// A pattern-valid `ApplicationArn` no create ever minted, for the not-found rule. Shaped
/// by the model's `ApplicationArn` pattern
/// `^arn:aws(-[a-z]{1,5}){0,3}:sso::\d{12}:application/(sso)?ins-[a-zA-Z0-9-.]{16}/apl-[a-zA-Z0-9]{16}$`.
const UNKNOWN_APPLICATION_ARN: &str =
    "arn:aws:sso::123456789012:application/ssoins-0000000000000000/apl-0000000000000000";

/// One row: a principal to grant, in one of the forms the model's `PrincipalId` pattern
/// and `PrincipalType` enum admit, and how many times the grant is repeated after the
/// first one.
struct Row {
    label: &'static str,
    principal_type: &'static str,
    principal_id: &'static str,
    repeats: usize,
}

/// The table. The rows span both values of `PrincipalType` and every form of
/// `PrincipalId` — SOURCE for both:
/// `smithy_query(query="operation", operation="CreateApplicationAssignment")`, whose
/// `PrincipalType` is `GROUP | USER` and whose `PrincipalId` is
/// `^([0-9a-f]{10}-|)[A-Fa-f0-9]{8}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{12}$`
/// of length 1..47 — and they differ in the NUMBER of repeats, from a single retry to a
/// reconcile loop that keeps re-asserting the grant.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "a lowercase-hex USER GUID, repeated once",
            principal_type: "USER",
            principal_id: "7b9c1d2e-3f40-4a51-8b62-c7d8e9f01234",
            repeats: 1,
        },
        Row {
            label: "an uppercase-hex USER GUID, repeated three times",
            principal_type: "USER",
            principal_id: "1D2E3F40-5A6B-4C7D-8E9F-0A1B2C3D4E5F",
            repeats: 3,
        },
        Row {
            label: "a lowercase-hex GROUP GUID, repeated twice",
            principal_type: "GROUP",
            principal_id: "2c3d4e5f-6071-4829-8a3b-4c5d6e7f8091",
            repeats: 2,
        },
        Row {
            // The prefixed form at the member's 47-character maximum.
            label: "a prefixed GROUP id at the 47-character limit, repeated five times",
            principal_type: "GROUP",
            principal_id: "1f2e3d4c5b-0a1b2c3d-4e5f-4071-8293-a4b5c6d7e8f9",
            repeats: 5,
        },
        Row {
            label: "a prefixed USER id at the 47-character limit, repeated once",
            principal_type: "USER",
            principal_id: "00112233aa-ffeeddcc-bbaa-4998-8776-554433221100",
            repeats: 1,
        },
    ]
}

impl Row {
    /// The row's assignment as its identity: what the rest of the tuple is once the
    /// application is fixed.
    fn identity(&self) -> String {
        format!("{}#{}", self.principal_type, self.principal_id)
    }
}

/// Assert one grant was SERVED: HTTP 200 carrying the modelled empty body, which is the
/// whole answer `CreateApplicationAssignment` declares.
///
/// Shared with the property suite, so both forms of the rule pin the same answer.
pub(crate) fn assert_granted(case: &str, what: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 200,
        "case {case:?}: {what} must answer 200, got {status} {body}"
    );
    assert!(
        body.is_null() || body.as_object().is_some_and(Map::is_empty),
        "case {case:?}: {what} must answer the modelled empty body, got {body}"
    );
}

/// Delete one direct application assignment, the way a client does.
async fn delete_assignment(
    router: &MultiServiceRouter,
    application_arn: &str,
    principal_type: &str,
    principal_id: &str,
) -> (u16, Value) {
    call(
        router,
        "DeleteApplicationAssignment",
        &json!({
            "ApplicationArn": application_arn,
            "PrincipalType": principal_type,
            "PrincipalId": principal_id,
        }),
    )
    .await
}

/// The assignments the application holds, read over the wire, sorted so the comparison is
/// about the SET rather than about a page order.
///
/// Shared with the property suite: one reader for the state both forms of the rule assert.
pub(crate) async fn held(
    router: &MultiServiceRouter,
    application_arn: &str,
    case: &str,
) -> Vec<String> {
    let (status, body) = call(
        router,
        "ListApplicationAssignments",
        &json!({"ApplicationArn": application_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListApplicationAssignments expected 200, got {status} {body}"
    );
    let mut found: Vec<String> = body["ApplicationAssignments"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: ApplicationAssignments must report a list, got {body}")
        })
        .iter()
        .map(|item| {
            format!(
                "{}#{}",
                item["PrincipalType"].as_str().unwrap_or_default(),
                item["PrincipalId"].as_str().unwrap_or_default()
            )
        })
        .collect();
    found.sort();
    found
}

/// Mint one application to grant assignments on.
async fn application(router: &MultiServiceRouter, instance_arn: &str, name: &str) -> String {
    minted_application(router, instance_arn, &Application::new(name, name)).await
}

/// THE RULE: every repeat of a grant the application already holds is served with the
/// modelled empty 200, the application keeps exactly the one row the first grant made, and
/// the assignment stays readable.
#[tokio::test]
async fn a_repeated_grant_of_an_assignment_the_application_holds_converges() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(Arc::clone(&engine));
    let instance_arn = seeded_instance_arn(&router).await;
    for (index, row) in rows().into_iter().enumerate() {
        let application_arn =
            application(&router, &instance_arn, &format!("repeat-app-{index}")).await;

        let (status, body) = grant(
            &router,
            &application_arn,
            row.principal_type,
            row.principal_id,
        )
        .await;
        assert_granted(row.label, "the first grant", status, &body);
        let stored_rows = with_store(&engine, |store| store.application_assignments.len());

        for attempt in 2..=(row.repeats + 1) {
            let (status, body) = grant(
                &router,
                &application_arn,
                row.principal_type,
                row.principal_id,
            )
            .await;
            assert_granted(row.label, &format!("grant #{attempt}"), status, &body);
            let found = held(&router, &application_arn, row.label).await;
            assert_eq!(
                found,
                vec![row.identity()],
                "case {:?}: after grant #{attempt} the application must still hold exactly \
                 {:?}, got {found:?}",
                row.label,
                row.identity()
            );
            assert_eq!(
                with_store(&engine, |store| store.application_assignments.len()),
                stored_rows,
                "case {:?}: grant #{attempt} must store no second row",
                row.label
            );
            // The repeat left the grant readable: the access the first grant made is
            // neither removed nor duplicated.
            let (status, body) = describe_application_assignment(
                &router,
                &application_arn,
                row.principal_type,
                row.principal_id,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: after grant #{attempt} the assignment must still describe, got \
                 {status} {body}",
                row.label
            );
        }
    }
}

/// The same rule when the repeats are in flight TOGETHER rather than one after another:
/// five grants of one tuple, all awaited concurrently, every one served, one assignment
/// stored.
#[tokio::test]
async fn concurrent_grants_of_one_tuple_are_all_served_and_leave_one_assignment() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(Arc::clone(&engine));
    let instance_arn = seeded_instance_arn(&router).await;
    for (index, row) in rows().into_iter().enumerate() {
        let application_arn =
            application(&router, &instance_arn, &format!("racing-app-{index}")).await;

        let answers = five_at_once(&router, &application_arn, &row).await;
        for (attempt, (status, body)) in answers.iter().enumerate() {
            assert_granted(
                row.label,
                &format!("concurrent grant #{}", attempt + 1),
                *status,
                body,
            );
        }

        let found = held(&router, &application_arn, row.label).await;
        assert_eq!(
            found,
            vec![row.identity()],
            "case {:?}: five concurrent grants must leave exactly {:?}, got {found:?}",
            row.label,
            row.identity()
        );
        assert_eq!(
            with_store(&engine, |store| store
                .application_assignments
                .iter()
                .filter(|stored| stored.application_arn == application_arn)
                .count()),
            1,
            "case {:?}: five concurrent grants must store ONE row for that application",
            row.label
        );
    }
}

/// Five grants of one row's tuple, all in flight at once over the same router.
async fn five_at_once(
    router: &MultiServiceRouter,
    application_arn: &str,
    row: &Row,
) -> Vec<(u16, Value)> {
    let one = || grant(router, application_arn, row.principal_type, row.principal_id);
    let (first, second, third, fourth, fifth) = tokio::join!(one(), one(), one(), one(), one());
    vec![first, second, third, fourth, fifth]
}

/// The grant is about the assignment THIS application holds: the same principal is granted
/// on an application that does not hold it, and a repeat on either application converges
/// on that application's own single row.
#[tokio::test]
async fn the_same_principal_is_granted_on_each_application_independently() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    for (index, row) in rows().into_iter().enumerate() {
        let holder = application(&router, &instance_arn, &format!("holder-app-{index}")).await;
        let other = application(&router, &instance_arn, &format!("other-app-{index}")).await;

        for (case, application_arn) in [("the holder", &holder), ("another application", &other)] {
            let (status, body) = grant(
                &router,
                application_arn,
                row.principal_type,
                row.principal_id,
            )
            .await;
            assert_granted(row.label, &format!("the grant on {case}"), status, &body);
            let found = held(&router, application_arn, row.label).await;
            assert_eq!(
                found,
                vec![row.identity()],
                "case {:?}: {case} must hold {:?}, got {found:?}",
                row.label,
                row.identity()
            );
        }

        // Each application now holds the assignment, so the repeat converges on BOTH, and
        // neither application's row is touched by the other's repeat.
        for (case, application_arn) in [("the holder", &holder), ("another application", &other)] {
            let (status, body) = grant(
                &router,
                application_arn,
                row.principal_type,
                row.principal_id,
            )
            .await;
            assert_granted(row.label, &format!("the repeat on {case}"), status, &body);
            let found = held(&router, application_arn, row.label).await;
            assert_eq!(
                found,
                vec![row.identity()],
                "case {:?}: after the repeat on {case} it must still hold {:?}, got {found:?}",
                row.label,
                row.identity()
            );
        }
    }
}

/// A delete removes the assignment, so the next grant CREATES it again rather than
/// converging on a row that is gone — and the repeat after it converges once more.
#[tokio::test]
async fn an_assignment_a_delete_removed_is_granted_afresh() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    for (index, row) in rows().into_iter().enumerate() {
        let application_arn =
            application(&router, &instance_arn, &format!("cycle-app-{index}")).await;

        // Two full grant / repeat / delete cycles, so the rule holds whatever the
        // application's history is.
        for cycle in 1..=2 {
            let (status, body) = grant(
                &router,
                &application_arn,
                row.principal_type,
                row.principal_id,
            )
            .await;
            assert_granted(
                row.label,
                &format!("the grant of cycle {cycle}"),
                status,
                &body,
            );

            let (status, body) = grant(
                &router,
                &application_arn,
                row.principal_type,
                row.principal_id,
            )
            .await;
            assert_granted(
                row.label,
                &format!("the repeat of cycle {cycle}"),
                status,
                &body,
            );
            let found = held(&router, &application_arn, row.label).await;
            assert_eq!(
                found,
                vec![row.identity()],
                "case {:?}: cycle {cycle} must hold exactly {:?}, got {found:?}",
                row.label,
                row.identity()
            );

            let (status, body) = delete_assignment(
                &router,
                &application_arn,
                row.principal_type,
                row.principal_id,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: the delete of cycle {cycle} expected 200, got {status} {body}",
                row.label
            );
            let found = held(&router, &application_arn, row.label).await;
            assert!(
                found.is_empty(),
                "case {:?}: after the delete of cycle {cycle} the application must hold \
                 nothing, got {found:?}",
                row.label
            );
        }
    }
}

/// Converging on a grant that exists is not the same as serving every grant: an
/// `ApplicationArn` no create minted is still refused `ResourceNotFoundException`, however
/// often it is sent.
#[tokio::test]
async fn a_grant_on_an_application_nothing_holds_is_still_refused() {
    let router = router();
    for row in rows() {
        for attempt in 1..=2 {
            let (status, body) = grant(
                &router,
                UNKNOWN_APPLICATION_ARN,
                row.principal_type,
                row.principal_id,
            )
            .await;
            assert_refused(
                row.label,
                &format!("grant #{attempt} on an unknown application"),
                "ResourceNotFoundException",
                status,
                &body,
            );
        }
    }
}
