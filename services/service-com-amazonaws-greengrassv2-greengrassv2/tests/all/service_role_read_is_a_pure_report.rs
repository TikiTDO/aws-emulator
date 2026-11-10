// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests pinning `GetServiceRoleForAccount` as a pure REPORT of the one
//! Greengrass service-role association an account holds.
//!
//! The rule under test: `GetServiceRoleForAccount` answers 200 with the role ARN and the
//! instant of the write that established the association the account currently holds, or
//! with neither member while it holds none — and reading NEVER changes what a later
//! caller sees, however many reads are made and wherever in the lifecycle they fall: a
//! read neither creates, re-stamps nor consumes the association, so the role and the
//! instant every read reports are the ones the write answered and a later disassociation
//! still finds the association it would have found without the reads.
//!
//! The sibling suite `tests/all/service_role_association.rs` holds the WRITE axis of the
//! same single association — that associating replaces, that disassociating removes, and
//! that each instant is the RFC 3339 UTC form — and owns the harness this suite drives
//! (the router, the three calls, the body readers, the clock and the role-ARN catalog), so
//! a service-role request is built and read exactly one way across this crate. The axis
//! here is the READ: how many reads the lifecycle is observed through, and that observing
//! it changes nothing.
//!
//! The rows differ along what the rule generalises over: the number of reads interleaved
//! at every point of the lifecycle — none, one and several — and which pair of the shared
//! catalog's role-ARN forms each row associates.
//!
//! Every assertion drives the real `restJson1` front door: the request is built the way
//! an SDK builds it, dispatched through `MultiServiceRouter`, and judged on the HTTP
//! status and the JSON body, so an association that was reported but never persisted —
//! or one a read quietly disturbed — fails here.

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::service_role_association::{
    associate, disassociate, emulator, get_role, instant_within, now_seconds, role_arn_rows, text,
};

/// The output members `GetServiceRoleForAccount` reports, by the wire names the model
/// gives them (`jsonName` of `associatedAt` and `roleArn` in
/// `smithy_query(query="operation",operation="GetServiceRoleForAccount")`), sorted — so a
/// read that invents a member fails here too.
const REPORTED_MEMBERS: [&str; 2] = ["AssociatedAt", "RoleArn"];

/// The member names a JSON object carries, sorted; empty for any other body.
fn members(body: &Value) -> Vec<String> {
    let mut names: Vec<String> = body
        .as_object()
        .map(|object| object.keys().cloned().collect())
        .unwrap_or_default();
    names.sort();
    names
}

/// Read the association `reads` times and assert every read reports exactly the role and
/// instant `expected` names — or, for `None`, that it reports no association at all.
/// That the LAST read still agrees with the first is the pure-report rule: a read that
/// created, re-stamped or consumed the association would show up here.
async fn reads_report(
    router: &MultiServiceRouter,
    case: &str,
    stage: &str,
    reads: usize,
    expected: Option<(&str, &str)>,
) {
    for read in 1..=reads {
        let (status, got) = get_role(router).await;
        assert_eq!(
            status, 200,
            "case {case:?} {stage} read {read}: expected 200 from \
             GetServiceRoleForAccount, got {status} {got}"
        );
        match expected {
            Some((role_arn, associated_at)) => {
                assert_eq!(
                    (text(&got, "RoleArn"), text(&got, "AssociatedAt")),
                    (role_arn.to_string(), associated_at.to_string()),
                    "case {case:?} {stage} read {read}: expected the association the \
                     write answered, got {got}"
                );
                assert_eq!(
                    members(&got),
                    REPORTED_MEMBERS.map(String::from).to_vec(),
                    "case {case:?} {stage} read {read}: expected exactly the reported \
                     members, got {got}"
                );
            }
            None => assert_eq!(
                members(&got),
                Vec::<String>::new(),
                "case {case:?} {stage} read {read}: expected a success naming no \
                 association, got {got}"
            ),
        }
    }
}

/// Associate `role_arn` and answer the `associatedAt` text the write reported, having
/// checked the status and that the instant is a reading of the clock in the window around
/// the call rather than a fabricated or echoed value.
async fn associated_at(router: &MultiServiceRouter, case: &str, role_arn: &str) -> String {
    let opened = now_seconds();
    let (status, written) = associate(router, role_arn).await;
    let closed = now_seconds();
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from AssociateServiceRoleToAccount, got {status} \
         {written}"
    );
    let reported = text(&written, "AssociatedAt");
    instant_within(case, &reported, opened, closed);
    reported
}

/// How many reads a row interleaves at every point of the lifecycle, and which two entries
/// of the shared role-ARN catalog it associates. The read count is what this suite's rule
/// generalises over (a report is repeatable); the ARN forms come from
/// `service_role_association::role_arn_rows`, which names the model as their source.
fn rows() -> Vec<(&'static str, usize, usize, usize)> {
    vec![
        // The control row: no read at all between the writes, so the lifecycle the sibling
        // suite pins is what every other row adds reads to.
        ("no reads interleaved at all", 0, 0, 1),
        ("one read at each point", 1, 1, 2),
        ("a handful of reads at each point", 3, 3, 4),
        ("many reads at each point", 11, 4, 0),
    ]
}

#[tokio::test]
async fn reading_the_service_role_reports_it_without_changing_it() {
    let catalog = role_arn_rows();
    for (case, reads, first, second) in rows() {
        let role = |index: usize| -> String {
            catalog
                .get(index)
                .map(|(_, role_arn)| role_arn.clone())
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the shared role-ARN catalog holds no row {index}")
                })
        };
        let first_role = role(first);
        let second_role = role(second);
        let router = emulator();

        // Nothing is associated yet, so every read answers a success naming no
        // association — and none of those reads may bring one into being.
        reads_report(&router, case, "before any association", reads, None).await;

        let first_at = associated_at(&router, case, &first_role).await;
        reads_report(
            &router,
            case,
            "with the first role associated",
            reads,
            Some((&first_role, &first_at)),
        )
        .await;

        // Replacing the association is the only thing that re-stamps it: after the second
        // write every read reports the second role and the second instant, and the reads
        // in between left the first write's instant untouched.
        let second_at = associated_at(&router, case, &second_role).await;
        reads_report(
            &router,
            case,
            "with the second role associated",
            reads,
            Some((&second_role, &second_at)),
        )
        .await;

        // The reads consumed nothing: this disassociation still finds the association the
        // second write left, so it reports the instant it removed it at.
        let opened = now_seconds();
        let (status, removed) = disassociate(&router).await;
        let closed = now_seconds();
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from DisassociateServiceRoleFromAccount, got \
             {status} {removed}"
        );
        instant_within(case, &text(&removed, "DisassociatedAt"), opened, closed);

        // And the account holds none again, however many times that is read — and the
        // reads created nothing for a second disassociation to remove.
        reads_report(&router, case, "after disassociating", reads, None).await;
        let (status, again) = disassociate(&router).await;
        assert_eq!(
            (status, text(&again, "DisassociatedAt").as_str()),
            (200, ""),
            "case {case:?}: expected a success that disassociated nothing, got {status} \
             {again}"
        );
        reads_report(&router, case, "after the second disassociation", reads, None).await;
    }
}

#[tokio::test]
async fn the_instant_a_read_reports_is_the_one_the_write_answered() {
    let router = emulator();
    let (case, role_arn) = role_arn_rows()
        .first()
        .cloned()
        .expect("the shared role-ARN catalog is not empty");

    // One association, read a whole second after the write: a report that read the clock
    // instead of the stored association would drift off the write's instant, because the
    // reported instant carries whole-second resolution. 1.1 s is the smallest delay that
    // guarantees that drift.
    let associated = associated_at(&router, case, &role_arn).await;
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
    reads_report(
        &router,
        case,
        "a second after the write",
        2,
        Some((&role_arn, &associated)),
    )
    .await;
}
