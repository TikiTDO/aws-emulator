// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how this service answers an Aurora Serverless v2 scaling
//! configuration whose capacity FLOOR is above its CEILING.
//!
//! The rule under test: a `ServerlessV2ScalingConfiguration` whose `MinCapacity` is greater
//! than its `MaxCapacity` names a capacity floor the member does not accept, so every
//! operation that carries the member refuses the call at HTTP 400 with `Error/Type`
//! `Sender`, `Error/Code` `InvalidParameterValue` and ONE fixed sentence that interpolates
//! nothing — neither the capacities the request submitted nor the member path they arrived
//! on — and the refusal is TOTAL: the cluster a rescale addressed keeps the range it had and
//! queues no pending change, and a refused create or restore leaves no cluster behind. A
//! pair whose floor EQUALS its ceiling is inside the rule and is served, as is any pair
//! whose floor is below it.
//!
//! The tables span what the rule generalises over rather than one request: the three
//! operations that carry the member and are served here (`CreateDBCluster`,
//! `ModifyDBCluster`, `RestoreDBClusterFromSnapshot` — `RestoreDBClusterToPointInTime` and
//! `RestoreDBClusterFromS3` carry it too and are still stubs), two Aurora engines, and four
//! inverted pairs: the published capacity bounds of the cluster's own engine version turned
//! around (read from `DescribeDBEngineVersions`' `ServerlessV2FeaturesSupport`, so the
//! widest inversion is taken at each published bound rather than at a figure written here),
//! two adjacent half-steps — the smallest inversion the member admits — whole ACUs, and a
//! half-step floor above a whole-ACU ceiling.
//!
//! Every call goes over the wire through the harness in `db_instances.rs`, so each assertion
//! reads the XML the emulator itself answered with.

use smithy_server_runtime::MultiServiceRouter;

use crate::db_clusters::describe_one;
use crate::db_instances::{
    call, emulator, expect_error, expect_ok, subtrees, text, wait_for_settle,
};

/// The whole sentence an inverted capacity range is refused with, spelled out HERE rather
/// than taken from the code under test, so the table pins the wording instead of re-stating
/// whatever the implementation happens to say. It has no variable part at all: it names
/// neither capacity and no member.
const FLOOR_ABOVE_CEILING: &str =
    "Serverless v2 minimum capacity must be less than or equal to maximum capacity";

/// The Query API's own code for a member value the service will not serve.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// Who the Query API's error envelope blames for a request it will not serve.
const SENDER: &str = "Sender";

/// The modelled fault a cluster identifier that resolves to nothing is answered with, which
/// is what says a refused create or restore left nothing behind.
const CLUSTER_NOT_FOUND: &str = "DBClusterNotFoundFault";

/// The increment a capacity value is specified in: "You can specify ACU values in half-step
/// increments, such as 8, 8.5, 9, and so on"
/// (`smithy_query(query="shape", shape="ServerlessV2ScalingConfiguration")`, member
/// `MinCapacity`), so a pair built from a published bound stays on the grid the member
/// admits and the refusal under test is the inversion rather than the step.
const CAPACITY_STEP: f64 = 0.5;

/// The capacity range the clusters below are created with — a floor and a ceiling every
/// Aurora Serverless v2 engine version publishes room for, so the refused rescales all start
/// from the same live configuration.
const LIVE_MIN: f64 = 0.5;
const LIVE_MAX: f64 = 1.0;

/// One ACU value as a request spells it and as the XML wire reports it back: a double always
/// carries a fractional part (`4.0`, never `4`).
fn acu(value: f64) -> String {
    format!("{value:?}")
}

/// The `ServerlessV2ScalingConfiguration` members of one request.
fn scaling_params(min: f64, max: f64) -> Vec<(&'static str, String)> {
    vec![
        ("ServerlessV2ScalingConfiguration.MinCapacity", acu(min)),
        ("ServerlessV2ScalingConfiguration.MaxCapacity", acu(max)),
    ]
}

/// Which operation carries the scaling configuration a row is refused for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Carrier {
    /// `CreateDBCluster`, provisioning a cluster at that range.
    Create,
    /// `ModifyDBCluster`, rescaling a live cluster to it.
    Modify,
    /// `RestoreDBClusterFromSnapshot`, overriding the snapshot's range with it.
    Restore,
}

impl Carrier {
    fn action(self) -> &'static str {
        match self {
            Carrier::Create => "CreateDBCluster",
            Carrier::Modify => "ModifyDBCluster",
            Carrier::Restore => "RestoreDBClusterFromSnapshot",
        }
    }
}

/// The three operations of this service that carry the member and serve a request.
const CARRIERS: [Carrier; 3] = [Carrier::Create, Carrier::Modify, Carrier::Restore];

/// One subject the whole table is run against: a live cluster of one engine, and the
/// snapshot of it a restore names.
struct Subject {
    label: &'static str,
    engine: &'static str,
    cluster: &'static str,
    snapshot: &'static str,
}

/// Two of the engines whose versions publish an Aurora Serverless v2 capacity range.
const SUBJECTS: &[Subject] = &[
    Subject {
        label: "aurora-postgresql",
        engine: "aurora-postgresql",
        cluster: "emu-sv2-range-pg",
        snapshot: "emu-sv2-range-pg-snap",
    },
    Subject {
        label: "aurora-mysql",
        engine: "aurora-mysql",
        cluster: "emu-sv2-range-my",
        snapshot: "emu-sv2-range-my-snap",
    },
];

/// Create `subject`'s cluster at the live range and hand back the engine version RDS
/// resolved for it.
async fn create_live_cluster(router: &MultiServiceRouter, subject: &Subject) -> String {
    let mut request: Vec<(&'static str, String)> = vec![
        ("DBClusterIdentifier", subject.cluster.to_string()),
        ("Engine", subject.engine.to_string()),
        ("MasterUsername", "emuadmin".to_string()),
        ("MasterUserPassword", "a-strong-password".to_string()),
    ];
    request.extend(scaling_params(LIVE_MIN, LIVE_MAX));
    let created = expect_ok(
        subject.label,
        call(router, "CreateDBCluster", &request).await,
    );
    text(&created, "EngineVersion")
}

/// The capacity range `version` of `engine` publishes, as `DescribeDBEngineVersions` reports
/// it on that version's `ServerlessV2FeaturesSupport`.
async fn published_range(
    router: &MultiServiceRouter,
    case: &str,
    engine: &str,
    version: &str,
) -> (f64, f64) {
    let body = expect_ok(
        case,
        call(
            router,
            "DescribeDBEngineVersions",
            &[("Engine", engine), ("EngineVersion", version)],
        )
        .await,
    );
    let support = subtrees(&body, "ServerlessV2FeaturesSupport")
        .into_iter()
        .next()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: an Aurora Serverless v2 engine version must publish its \
                 capacity range, got {body}"
            )
        });
    let read = |member: &str| -> f64 {
        let value = text(&support, member);
        value.parse().unwrap_or_else(|_| {
            panic!("case {case:?}: {member} must be a number, got {value:?}")
        })
    };
    (read("MinCapacity"), read("MaxCapacity"))
}

/// The capacity range a cluster envelope reports, read off the subtree the member occupies
/// so a serverless v1 `ScalingConfigurationInfo` beside it cannot be mistaken for it.
fn reported_range(case: &str, cluster: &str) -> (String, String) {
    let scaling = subtrees(cluster, "ServerlessV2ScalingConfiguration")
        .into_iter()
        .next()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a reported Serverless v2 scaling configuration, \
                 got {cluster}"
            )
        });
    (text(&scaling, "MinCapacity"), text(&scaling, "MaxCapacity"))
}

/// Assert that `identifier` names no cluster at all: what a refused create or restore must
/// leave behind.
async fn expect_no_such_cluster(router: &MultiServiceRouter, case: &str, identifier: &str) {
    expect_error(
        case,
        call(
            router,
            "DescribeDBClusters",
            &[("DBClusterIdentifier", identifier)],
        )
        .await,
        404,
        CLUSTER_NOT_FOUND,
    );
}

/// One inverted pair the table carries.
struct InvertedPair {
    case: String,
    min: f64,
    max: f64,
}

/// The inverted pairs for a cluster whose engine version publishes `floor` to `ceiling`:
/// both published bounds turned around, two adjacent half-steps, whole ACUs, and a half-step
/// floor above a whole-ACU ceiling. Every value is inside the published range and on the
/// half-step grid, so the inversion is the only thing any row breaks.
fn inverted_pairs(floor: f64, ceiling: f64) -> Vec<InvertedPair> {
    vec![
        InvertedPair {
            case: format!("the published bounds inverted ({ceiling:?} down to {floor:?})"),
            min: ceiling,
            max: floor,
        },
        InvertedPair {
            case: "two adjacent half-steps, inverted".to_string(),
            min: floor + CAPACITY_STEP,
            max: floor,
        },
        InvertedPair {
            case: "whole ACUs, four above one".to_string(),
            min: 4.0,
            max: 1.0,
        },
        InvertedPair {
            case: "a half-step floor above a whole-ACU ceiling".to_string(),
            min: 8.5,
            max: 8.0,
        },
    ]
}

#[tokio::test]
async fn an_inverted_capacity_range_is_refused_the_same_way_by_every_operation_that_carries_it()
{
    let router = emulator();

    // The live clusters, settled together, and a snapshot of each for the restore rows.
    let mut versions = Vec::new();
    for subject in SUBJECTS {
        versions.push(create_live_cluster(&router, subject).await);
    }
    wait_for_settle();
    for subject in SUBJECTS {
        expect_ok(
            subject.label,
            call(
                &router,
                "CreateDBClusterSnapshot",
                &[
                    ("DBClusterIdentifier", subject.cluster),
                    ("DBClusterSnapshotIdentifier", subject.snapshot),
                ],
            )
            .await,
        );
    }
    wait_for_settle();

    for (subject, version) in SUBJECTS.iter().zip(&versions) {
        let (floor, ceiling) =
            published_range(&router, subject.label, subject.engine, version).await;
        assert!(
            floor < ceiling,
            "case {:?}: expected a published capacity range to rest on, got {floor:?} to \
             {ceiling:?}",
            subject.label
        );

        for (index, pair) in inverted_pairs(floor, ceiling).into_iter().enumerate() {
            for carrier in CARRIERS {
                let case = format!(
                    "{} / {} / {}",
                    subject.label,
                    carrier.action(),
                    pair.case
                );
                // The identifier a refused create or restore would have provisioned, so the
                // row can prove nothing was left behind under it.
                let fresh = format!(
                    "{}-{}{index}",
                    subject.cluster,
                    match carrier {
                        Carrier::Create => "c",
                        Carrier::Modify => "m",
                        Carrier::Restore => "r",
                    }
                );
                let mut request: Vec<(&'static str, String)> = match carrier {
                    Carrier::Create => vec![
                        ("DBClusterIdentifier", fresh.clone()),
                        ("Engine", subject.engine.to_string()),
                    ],
                    Carrier::Modify => vec![
                        ("DBClusterIdentifier", subject.cluster.to_string()),
                        ("ApplyImmediately", "true".to_string()),
                    ],
                    Carrier::Restore => vec![
                        ("DBClusterIdentifier", fresh.clone()),
                        ("SnapshotIdentifier", subject.snapshot.to_string()),
                        ("Engine", subject.engine.to_string()),
                    ],
                };
                request.extend(scaling_params(pair.min, pair.max));

                let refused = expect_error(
                    &case,
                    call(&router, carrier.action(), &request).await,
                    400,
                    INVALID_PARAMETER_VALUE,
                );
                let blamed = text(&refused, "Type");
                assert_eq!(
                    blamed, SENDER,
                    "case {case:?}: expected the sender to be blamed, got {blamed:?}"
                );
                let message = text(&refused, "Message");
                assert_eq!(
                    message, FLOOR_ABOVE_CEILING,
                    "case {case:?}: expected the fixed sentence {FLOOR_ABOVE_CEILING:?}, got \
                     {message:?}"
                );
                // The sentence is fixed: the two capacities this row submitted are not in
                // it, and neither is the member path they arrived on.
                for value in [pair.min, pair.max] {
                    let rendered = acu(value);
                    assert!(
                        !message.contains(&rendered),
                        "case {case:?}: the refusal must not interpolate the submitted \
                         capacity {rendered:?}, got {message:?}"
                    );
                }
                for member in [
                    "ServerlessV2ScalingConfiguration",
                    "MinCapacity",
                    "MaxCapacity",
                ] {
                    assert!(
                        !message.contains(member),
                        "case {case:?}: the refusal must not name {member:?}, got {message:?}"
                    );
                }

                // The refusal is total. A rescale left the live cluster exactly as it was —
                // the range it was created with, no pending change queued, and the status it
                // already had; a create or a restore left no cluster under the identifier it
                // named.
                match carrier {
                    Carrier::Modify => {
                        let live = describe_one(&router, &case, subject.cluster).await;
                        let reported = reported_range(&case, &live);
                        assert_eq!(
                            reported,
                            (acu(LIVE_MIN), acu(LIVE_MAX)),
                            "case {case:?}: expected the live range to be untouched, got \
                             {reported:?}"
                        );
                        assert!(
                            subtrees(&live, "PendingModifiedValues").is_empty(),
                            "case {case:?}: a refused rescale must queue no pending change, \
                             got {live}"
                        );
                        let status = text(&live, "Status");
                        assert_eq!(
                            status, "available",
                            "case {case:?}: expected the refused rescale to leave the status \
                             alone, got {status:?}"
                        );
                    }
                    Carrier::Create | Carrier::Restore => {
                        expect_no_such_cluster(&router, &case, &fresh).await;
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn a_capacity_floor_at_or_below_its_ceiling_is_served_by_every_one_of_them() {
    let router = emulator();

    for subject in SUBJECTS {
        create_live_cluster(&router, subject).await;
    }
    wait_for_settle();
    for subject in SUBJECTS {
        expect_ok(
            subject.label,
            call(
                &router,
                "CreateDBClusterSnapshot",
                &[
                    ("DBClusterIdentifier", subject.cluster),
                    ("DBClusterSnapshotIdentifier", subject.snapshot),
                ],
            )
            .await,
        );
    }
    wait_for_settle();

    for subject in SUBJECTS {
        // The boundary the rule states: a floor EQUAL to the ceiling is "less than or equal",
        // so the rescale is taken and the cluster reports the range it asked for.
        let case = format!("{} / a floor equal to its ceiling", subject.label);
        let mut request: Vec<(&'static str, String)> = vec![
            ("DBClusterIdentifier", subject.cluster.to_string()),
            ("ApplyImmediately", "true".to_string()),
        ];
        request.extend(scaling_params(1.0, 1.0));
        expect_ok(&case, call(&router, "ModifyDBCluster", &request).await);
        let live = describe_one(&router, &case, subject.cluster).await;
        let reported = reported_range(&case, &live);
        assert_eq!(
            reported,
            (acu(1.0), acu(1.0)),
            "case {case:?}: expected the equal pair to be the range in effect, got \
             {reported:?}"
        );

        // And the widening the inverted rows are the control for.
        let case = format!("{} / a widened range", subject.label);
        let mut request: Vec<(&'static str, String)> = vec![
            ("DBClusterIdentifier", subject.cluster.to_string()),
            ("ApplyImmediately", "true".to_string()),
        ];
        request.extend(scaling_params(1.0, 8.0));
        expect_ok(&case, call(&router, "ModifyDBCluster", &request).await);
        let live = describe_one(&router, &case, subject.cluster).await;
        let reported = reported_range(&case, &live);
        assert_eq!(
            reported,
            (acu(1.0), acu(8.0)),
            "case {case:?}: expected the widened range to be the one in effect, got \
             {reported:?}"
        );

        // The same two pairs on the other two operations that carry the member: a create at
        // an equal pair, and a restore that widens the snapshot's range.
        let case = format!("{} / a create at an equal pair", subject.label);
        let created = format!("{}-ok-create", subject.cluster);
        let mut request: Vec<(&'static str, String)> = vec![
            ("DBClusterIdentifier", created.clone()),
            ("Engine", subject.engine.to_string()),
        ];
        request.extend(scaling_params(8.5, 8.5));
        expect_ok(&case, call(&router, "CreateDBCluster", &request).await);
        let made = describe_one(&router, &case, &created).await;
        let reported = reported_range(&case, &made);
        assert_eq!(
            reported,
            (acu(8.5), acu(8.5)),
            "case {case:?}: expected the created cluster to carry the range it asked for, \
             got {reported:?}"
        );

        let case = format!("{} / a restore that widens the range", subject.label);
        let restored = format!("{}-ok-restore", subject.cluster);
        let mut request: Vec<(&'static str, String)> = vec![
            ("DBClusterIdentifier", restored.clone()),
            ("SnapshotIdentifier", subject.snapshot.to_string()),
            ("Engine", subject.engine.to_string()),
        ];
        request.extend(scaling_params(0.5, 16.0));
        expect_ok(
            &case,
            call(&router, "RestoreDBClusterFromSnapshot", &request).await,
        );
        let back = describe_one(&router, &case, &restored).await;
        let reported = reported_range(&case, &back);
        assert_eq!(
            reported,
            (acu(0.5), acu(16.0)),
            "case {case:?}: expected the restored cluster to carry the overridden range, got \
             {reported:?}"
        );
    }
}
