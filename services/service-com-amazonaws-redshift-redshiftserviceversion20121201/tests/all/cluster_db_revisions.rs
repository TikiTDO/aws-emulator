// SPDX-License-Identifier: CC0-1.0

//! `DescribeClusterDbRevisions` reports the database revision each cluster RUNS,
//! end to end over the awsQuery wire.
//!
//! The RULE every case here asserts: the operation reports one
//! `ClusterDbRevision` per cluster of the account — in the order the account
//! created them, a page at a time — and every value on it is read off that
//! cluster's own record, so what this operation says about a cluster and what
//! `DescribeClusters` says about it cannot disagree:
//!
//! * `ClusterIdentifier` and `CurrentDatabaseRevision` are the cluster's own
//!   identifier and the `ClusterRevisionNumber` its record reports;
//! * `DatabaseRevisionReleaseDate` is the instant the cluster was put on that
//!   revision, which is its `ClusterCreateTime`;
//! * a `ClusterIdentifier` that names no cluster is refused with
//!   `ClusterNotFoundFault` (404, `ClusterNotFound`), and naming none reports
//!   every cluster;
//! * the `MaxRecords` window is the documented 20..100 one, and `Marker` walks
//!   the same sequence.
//!
//! The rows differ along what the rule generalises over: how many nodes a
//! cluster has and of which type, whether it was launched into a subnet group of
//! its own or the account's default one, how many clusters the account has, the
//! page size asked for (each published bound and one past it) and the case of
//! the identifier asked about.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Answer, Emulator};

/// The admin password every cluster in this suite is created with.
const PASSWORD: &str = "TestPassw0rd123";

/// One cluster this suite creates, and what makes it differ from its siblings.
struct Row {
    case: &'static str,
    identifier: &'static str,
    /// Extra `CreateCluster` parameters: the node type, the cluster type and the
    /// node count this row differs on.
    params: &'static [(&'static str, &'static str)],
}

/// `CreateCluster` for one row.
async fn create(emulator: &Emulator, row: &Row) -> Answer {
    let mut params: Vec<(&str, &str)> = vec![
        ("ClusterIdentifier", row.identifier),
        ("MasterUsername", "adminuser"),
        ("MasterUserPassword", PASSWORD),
    ];
    params.extend_from_slice(row.params);
    emulator.call("CreateCluster", &params).await
}

/// `DescribeClusterDbRevisions` with these parameters.
async fn revisions(emulator: &Emulator, params: &[(&str, &str)]) -> Answer {
    emulator.call("DescribeClusterDbRevisions", params).await
}

/// What `DescribeClusters` reports for one cluster, as the pair this suite
/// compares a revision record against.
async fn cluster_facts(emulator: &Emulator, identifier: &str) -> (String, String) {
    let described = emulator
        .call("DescribeClusters", &[("ClusterIdentifier", identifier)])
        .await;
    assert_eq!(
        described.status, 200,
        "case {identifier}: expected the cluster read served, got {} with body {}",
        described.status, described.body
    );
    let revision = described.text("ClusterRevisionNumber").unwrap_or_else(|| {
        panic!(
            "case {identifier}: expected a ClusterRevisionNumber, got body {}",
            described.body
        )
    });
    let created = described.text("ClusterCreateTime").unwrap_or_else(|| {
        panic!(
            "case {identifier}: expected a ClusterCreateTime, got body {}",
            described.body
        )
    });
    (revision, created)
}

/// Every cluster of the account is reported, each carrying its OWN revision and
/// the instant it was put on it.
#[tokio::test]
async fn every_cluster_reports_the_revision_its_record_carries() {
    let emulator = Emulator::new();

    let rows: [Row; 3] = [
        Row {
            case: "a single-node ra3 cluster",
            identifier: "revisions-single",
            params: &[("NodeType", "ra3.xlplus"), ("ClusterType", "single-node")],
        },
        Row {
            case: "a two-node ra3 cluster",
            identifier: "revisions-two-node",
            params: &[
                ("NodeType", "ra3.xlplus"),
                ("ClusterType", "multi-node"),
                ("NumberOfNodes", "2"),
            ],
        },
        Row {
            case: "a cluster of another node type",
            identifier: "revisions-dc2",
            params: &[("NodeType", "dc2.large"), ("ClusterType", "single-node")],
        },
    ];

    for row in &rows {
        let created = create(&emulator, row).await;
        assert_eq!(
            created.status, 200,
            "case {}: expected the cluster create served, got {} with body {}",
            row.case, created.status, created.body
        );
    }

    // No filter: one record per cluster, in the order the account created them.
    let all = revisions(&emulator, &[]).await;
    assert_eq!(
        all.status, 200,
        "case every cluster: expected 200, got {} with body {}",
        all.status, all.body
    );
    let expected_identifiers: Vec<String> = rows
        .iter()
        .map(|row| row.identifier.to_string())
        .collect();
    assert_eq!(
        all_text(&all.body, "ClusterIdentifier"),
        expected_identifiers,
        "case every cluster: expected one record per cluster in creation order, got body {}",
        all.body
    );

    let reported_revisions = all_text(&all.body, "CurrentDatabaseRevision");
    let reported_dates = all_text(&all.body, "DatabaseRevisionReleaseDate");
    for (index, row) in rows.iter().enumerate() {
        let (revision, created_at) = cluster_facts(&emulator, row.identifier).await;
        assert_eq!(
            reported_revisions.get(index).map(String::as_str),
            Some(revision.as_str()),
            "case {}: expected the cluster's own ClusterRevisionNumber {revision:?}, got {:?}",
            row.case,
            reported_revisions.get(index)
        );
        assert_eq!(
            reported_dates.get(index).map(String::as_str),
            Some(created_at.as_str()),
            "case {}: expected the instant the cluster was put on that revision {created_at:?}, \
             got {:?}",
            row.case,
            reported_dates.get(index)
        );

        // Asking about ONE cluster reports that cluster alone, with the same
        // values the unfiltered read gave for it.
        let one = revisions(&emulator, &[("ClusterIdentifier", row.identifier)]).await;
        assert_eq!(
            one.status, 200,
            "case {}: expected 200, got {} with body {}",
            row.case, one.status, one.body
        );
        assert_eq!(
            all_text(&one.body, "ClusterIdentifier"),
            vec![row.identifier.to_string()],
            "case {}: expected only the cluster asked about, got body {}",
            row.case,
            one.body
        );
        assert_eq!(
            (
                one.text("CurrentDatabaseRevision"),
                one.text("DatabaseRevisionReleaseDate")
            ),
            (Some(revision.clone()), Some(created_at.clone())),
            "case {}: expected the same revision and release date as the listing, got {:?} / {:?}",
            row.case,
            one.text("CurrentDatabaseRevision"),
            one.text("DatabaseRevisionReleaseDate")
        );

        // This service publishes no later revision of a cluster's database than
        // the one it runs, so a provisioned cluster is on its latest revision
        // and offers no target to update to.
        assert!(
            all_text(&one.body, "DatabaseRevision").is_empty(),
            "case {}: expected no revision target for a cluster on its latest revision, got body \
             {}",
            row.case,
            one.body
        );
    }
}

/// An account with no cluster reports no revision, and a `ClusterIdentifier`
/// that names no cluster is refused with the operation's modelled fault.
#[tokio::test]
async fn an_identifier_that_names_no_cluster_is_refused() {
    let emulator = Emulator::new();

    let empty = revisions(&emulator, &[]).await;
    assert_eq!(
        (empty.status, all_text(&empty.body, "ClusterIdentifier")),
        (200, Vec::new()),
        "case an account with no cluster: expected an empty report, got {} with body {}",
        empty.status,
        empty.body
    );

    let created = create(
        &emulator,
        &Row {
            case: "the one cluster of the account",
            identifier: "revisions-present",
            params: &[("NodeType", "ra3.xlplus"), ("ClusterType", "single-node")],
        },
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case the one cluster of the account: expected the create served, got {} with body {}",
        created.status, created.body
    );

    // The rows differ along WHY the identifier names no cluster: never created,
    // created under a different name, and — "this parameter is case sensitive" —
    // the same name in another case.
    let rows: [(&str, &str); 3] = [
        ("never created", "no-such-cluster"),
        ("another cluster's name with a suffix", "revisions-present-2"),
        ("the right name in the wrong case", "Revisions-Present"),
    ];
    for (case, identifier) in rows {
        let answer = revisions(&emulator, &[("ClusterIdentifier", identifier)]).await;
        assert_eq!(
            (answer.status, answer.error_code().as_deref()),
            (404, Some("ClusterNotFound")),
            "case {case}: expected 404 / ClusterNotFound, got {} / {:?} with body {}",
            answer.status,
            answer.error_code(),
            answer.body
        );
        assert_eq!(
            answer.text("Message").as_deref(),
            Some(
                state::cluster_not_found_message(
                    state::ClusterNotFoundWording::IdentifierThenNotFound,
                    identifier
                )
                .as_str()
            ),
            "case {case}: expected the identifier the request named in the refusal, got {:?}",
            answer.text("Message")
        );
    }
}

/// The page the operation serves is the documented `MaxRecords` window's, and
/// `Marker` walks the same sequence of clusters exactly once.
#[tokio::test]
async fn the_page_follows_the_documented_max_records_window() {
    let emulator = Emulator::new();

    // One more cluster than the window's largest page, so the first page is full
    // and a marker is handed out for the rest.
    let count = state::MAX_RECORDS_MIN as usize + 1;
    let identifiers: Vec<String> = (0..count).map(|index| format!("paged-{index:02}")).collect();
    for identifier in &identifiers {
        let created = emulator
            .call(
                "CreateCluster",
                &[
                    ("ClusterIdentifier", identifier.as_str()),
                    ("NodeType", "ra3.xlplus"),
                    ("ClusterType", "single-node"),
                    ("MasterUsername", "adminuser"),
                    ("MasterUserPassword", PASSWORD),
                ],
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case seed {identifier}: expected the create served, got {} with body {}",
            created.status, created.body
        );
    }

    let page_size = state::MAX_RECORDS_MIN.to_string();
    let first = revisions(&emulator, &[("MaxRecords", &page_size)]).await;
    assert_eq!(
        all_text(&first.body, "ClusterIdentifier"),
        identifiers[..state::MAX_RECORDS_MIN as usize].to_vec(),
        "case the first page: expected the window's page in creation order, got body {}",
        first.body
    );
    let marker = first.text("Marker").unwrap_or_else(|| {
        panic!(
            "case the first page: expected a Marker while records remain, got body {}",
            first.body
        )
    });
    let second = revisions(
        &emulator,
        &[("MaxRecords", &page_size), ("Marker", marker.as_str())],
    )
    .await;
    assert_eq!(
        all_text(&second.body, "ClusterIdentifier"),
        identifiers[state::MAX_RECORDS_MIN as usize..].to_vec(),
        "case the second page: expected the rest of the sequence, got body {}",
        second.body
    );
    assert_eq!(
        second.text("Marker"),
        None,
        "case the second page: expected no Marker once the sequence is spent, got {:?}",
        second.text("Marker")
    );

    // Each published bound of the window, and one past it: only a page SMALLER
    // than the minimum is refused, while a page larger than the published
    // maximum is served, clamped to it.
    let rows: [(&str, i32, u16, usize); 4] = [
        ("the window's minimum", state::MAX_RECORDS_MIN, 200, 20),
        ("one below the minimum", state::MAX_RECORDS_MIN - 1, 400, 0),
        ("the window's maximum", state::MAX_RECORDS_MAX, 200, count),
        ("one past the maximum", state::MAX_RECORDS_MAX + 1, 200, count),
    ];
    for (case, max_records, status, served) in rows {
        let value = max_records.to_string();
        let answer = revisions(&emulator, &[("MaxRecords", &value)]).await;
        assert_eq!(
            answer.status, status,
            "case {case}: expected status {status}, got {} with body {}",
            answer.status, answer.body
        );
        if status == 200 {
            assert_eq!(
                all_text(&answer.body, "ClusterIdentifier").len(),
                served,
                "case {case}: expected {served} records, got body {}",
                answer.body
            );
        } else {
            assert_eq!(
                answer.error_code().as_deref(),
                Some("InvalidParameterValue"),
                "case {case}: expected InvalidParameterValue, got {:?} with body {}",
                answer.error_code(),
                answer.body
            );
        }
    }

    // "You can specify either the ClusterIdentifier parameter, or the marker
    // parameter, but not both."
    let both = revisions(
        &emulator,
        &[
            ("ClusterIdentifier", identifiers[0].as_str()),
            ("Marker", marker.as_str()),
        ],
    )
    .await;
    assert_eq!(
        (both.status, both.error_code().as_deref()),
        (400, Some("InvalidParameterCombination")),
        "case an identifier and a marker: expected 400 / InvalidParameterCombination, got {} / \
         {:?} with body {}",
        both.status,
        both.error_code(),
        both.body
    );
}
