// SPDX-License-Identifier: CC0-1.0

//! What a `SwapEnvironmentCNAMEs` leaves `CheckDNSAvailability` reporting, over
//! this service's awsQuery surface.
//!
//! RULE: an environment RESERVES the CNAME its `CNAMEPrefix` forms when it is
//! created, and `CheckDNSAvailability` reports a prefix as unavailable exactly
//! while a live environment holds that reservation — which it does only while it
//! still carries the name it reserved. A swap exchanges the two environments'
//! CNAMEs without carrying either reservation across, so each exchanged prefix is
//! reserved by nobody afterwards and both report available again; swapping back
//! restores both reservations. A prefix a live environment is SERVING is still a
//! name no new environment can take, because two environments cannot answer at
//! one host name.
//!
//! The rows span what the rule generalises over: which member the swap selects
//! each side by (id or name), which of the two prefixes is asked about, and a
//! prefix no swap touched.

use crate::lifecycle::{Emulator, PAST_DUE, SOLUTION_STACK, TEST_REGION};

/// The application every case is created under.
const APPLICATION: &str = "cname-app";

/// How a swap names the pair of environments it exchanges.
#[derive(Debug, Clone, Copy)]
enum SelectBy {
    Id,
    Name,
}

/// One row: how the pair is selected, and the two environments it swaps.
struct Row {
    case: &'static str,
    select_by: SelectBy,
    source: &'static str,
    destination: &'static str,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a pair selected by environment id",
            select_by: SelectBy::Id,
            source: "blue-by-id",
            destination: "green-by-id",
        },
        Row {
            case: "a pair selected by environment name",
            select_by: SelectBy::Name,
            source: "blue-by-name",
            destination: "green-by-name",
        },
    ]
}

/// The CNAME a prefix forms, as `state::environment_cname` builds it: the socket
/// this emulator serves the environment plane on, then the plane path that
/// prefix's environment is served at. Read off the emulator, because the port
/// that socket answers on is the core's to choose.
fn fully_qualified(emulator: &Emulator, prefix: &str) -> String {
    emulator.environment_address(TEST_REGION, prefix)
}

/// Create the environment `name` holding the CNAME `prefix` forms, and answer its
/// `EnvironmentId`.
async fn create(emulator: &Emulator, case: &str, name: &str, prefix: &str) -> String {
    let created = emulator
        .call(
            "CreateEnvironment",
            &[
                ("ApplicationName", APPLICATION),
                ("EnvironmentName", name),
                ("SolutionStackName", SOLUTION_STACK.as_str()),
                ("CNAMEPrefix", prefix),
            ],
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected 200 from CreateEnvironment, got {} and body {}",
        created.status, created.body
    );
    created.expect("EnvironmentId", case).to_string()
}

/// Whether `CheckDNSAvailability` reports `prefix` available.
async fn available(emulator: &Emulator, case: &str, prefix: &str) -> bool {
    let answer = emulator
        .call("CheckDNSAvailability", &[("CNAMEPrefix", prefix)])
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200 from CheckDNSAvailability, got {} and body {}",
        answer.status, answer.body
    );
    answer.expect("Available", case) == "true"
}

/// The CNAME `DescribeEnvironments` reports for the environment `environment_id`.
async fn reported_cname(emulator: &Emulator, case: &str, environment_id: &str) -> String {
    let described = emulator
        .call(
            "DescribeEnvironments",
            &[("EnvironmentIds.member.1", environment_id)],
        )
        .await;
    described
        .expect("Environments/member/CNAME", case)
        .to_string()
}

/// Swap the pair of environments `row` names.
async fn swap(emulator: &Emulator, row: &Row, source_id: &str, destination_id: &str) {
    let params: Vec<(&str, &str)> = match row.select_by {
        SelectBy::Id => vec![
            ("SourceEnvironmentId", source_id),
            ("DestinationEnvironmentId", destination_id),
        ],
        SelectBy::Name => vec![
            ("SourceEnvironmentName", row.source),
            ("DestinationEnvironmentName", row.destination),
        ],
    };
    let swapped = emulator.call("SwapEnvironmentCNAMEs", &params).await;
    assert_eq!(
        swapped.status, 200,
        "case {:?}: expected 200 from SwapEnvironmentCNAMEs, got {} and body {}",
        row.case, swapped.status, swapped.body
    );
}

#[tokio::test]
async fn a_swap_releases_the_prefixes_it_exchanges_and_a_second_swap_takes_them_back() {
    let emulator = Emulator::new();
    let created = emulator
        .call("CreateApplication", &[("ApplicationName", APPLICATION)])
        .await;
    assert_eq!(
        created.status, 200,
        "case \"setup\": expected 200 from CreateApplication, got {} and body {}",
        created.status, created.body
    );

    for row in rows() {
        let case = row.case;
        let source_prefix = format!("{}-prefix", row.source);
        let destination_prefix = format!("{}-prefix", row.destination);
        let source_id = create(&emulator, case, row.source, &source_prefix).await;
        let destination_id = create(&emulator, case, row.destination, &destination_prefix).await;
        // An untouched prefix, so what the swap changes is asserted against a
        // reservation no swap moved.
        let untouched_prefix = format!("{}-untouched", row.source);
        let untouched_id = create(
            &emulator,
            case,
            &format!("{}-untouched", row.source),
            &untouched_prefix,
        )
        .await;

        // Each environment reserved the name its own prefix formed, so neither
        // prefix is available before the swap.
        for prefix in [&source_prefix, &destination_prefix, &untouched_prefix] {
            assert!(
                !available(&emulator, case, prefix).await,
                "case {case:?}: {prefix} is reserved by the environment that took it"
            );
        }

        // The swap needs both environments Ready, which is what the clock owes.
        std::thread::sleep(PAST_DUE);
        swap(&emulator, &row, &source_id, &destination_id).await;

        // Each environment now answers at the OTHER's name...
        assert_eq!(
            reported_cname(&emulator, case, &source_id).await,
            fully_qualified(&emulator, &destination_prefix),
            "case {case:?}: the swap must hand the source the destination's CNAME"
        );
        assert_eq!(
            reported_cname(&emulator, case, &destination_id).await,
            fully_qualified(&emulator, &source_prefix),
            "case {case:?}: the swap must hand the destination the source's CNAME"
        );
        // ... and neither carries the name it reserved, so both exchanged
        // prefixes are reserved by nobody.
        for prefix in [&source_prefix, &destination_prefix] {
            assert!(
                available(&emulator, case, prefix).await,
                "case {case:?}: the swap released {prefix}, so it must report available"
            );
        }
        // The prefix no swap touched is still reserved by the environment that
        // took it.
        assert!(
            !available(&emulator, case, &untouched_prefix).await,
            "case {case:?}: a prefix no swap touched must stay reserved"
        );
        assert_eq!(
            reported_cname(&emulator, case, &untouched_id).await,
            fully_qualified(&emulator, &untouched_prefix),
            "case {case:?}: the untouched environment keeps its own CNAME"
        );

        // A name a live environment is SERVING cannot be taken by a new
        // environment, released reservation or not: two environments cannot
        // answer at one host name.
        let taken = emulator
            .call(
                "CreateEnvironment",
                &[
                    ("ApplicationName", APPLICATION),
                    ("EnvironmentName", &format!("{}-intruder", row.source)),
                    ("SolutionStackName", SOLUTION_STACK.as_str()),
                    ("CNAMEPrefix", source_prefix.as_str()),
                ],
            )
            .await;
        assert_eq!(
            taken.status, 400,
            "case {case:?}: expected the create to refuse a served name, got {} and body {}",
            taken.status, taken.body
        );
        let expected = format!(
            "DNS name ({}) is not available.",
            fully_qualified(&emulator, &source_prefix)
        );
        assert_eq!(
            taken.error_message(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            taken.error_message()
        );

        // The swap is its own inverse, so swapping back takes both reservations
        // up again.
        swap(&emulator, &row, &source_id, &destination_id).await;
        assert_eq!(
            reported_cname(&emulator, case, &source_id).await,
            fully_qualified(&emulator, &source_prefix),
            "case {case:?}: swapping twice must restore the source's CNAME"
        );
        for prefix in [&source_prefix, &destination_prefix] {
            assert!(
                !available(&emulator, case, prefix).await,
                "case {case:?}: {prefix} is reserved again once its environment carries it"
            );
        }
    }
}

/// Checking a prefix a swap released neither takes it nor gives it back: a report
/// changes nothing a later caller sees.
#[tokio::test]
async fn checking_a_released_prefix_changes_nothing() {
    let emulator = Emulator::new();
    let case = "a released prefix read twice";
    let created = emulator
        .call("CreateApplication", &[("ApplicationName", APPLICATION)])
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected 200 from CreateApplication, got {} and body {}",
        created.status, created.body
    );
    let source_id = create(&emulator, case, "blue", "blue-prefix").await;
    let destination_id = create(&emulator, case, "green", "green-prefix").await;
    std::thread::sleep(PAST_DUE);
    let swapped = emulator
        .call(
            "SwapEnvironmentCNAMEs",
            &[
                ("SourceEnvironmentId", source_id.as_str()),
                ("DestinationEnvironmentId", destination_id.as_str()),
            ],
        )
        .await;
    assert_eq!(
        swapped.status, 200,
        "case {case:?}: expected 200 from SwapEnvironmentCNAMEs, got {} and body {}",
        swapped.status, swapped.body
    );

    for round in 0..3 {
        assert!(
            available(&emulator, case, "blue-prefix").await,
            "case {case:?}: round {round} must still report the released name available"
        );
    }
    // And the environments are exactly as the swap left them.
    assert_eq!(
        reported_cname(&emulator, case, &source_id).await,
        fully_qualified(&emulator, "green-prefix"),
        "case {case:?}: the reads must not move either CNAME"
    );
    assert_eq!(
        reported_cname(&emulator, case, &destination_id).await,
        fully_qualified(&emulator, "blue-prefix"),
        "case {case:?}: the reads must not move either CNAME"
    );
}
