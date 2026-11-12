// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a proxy's target registry: `RegisterDBProxyTargets`,
//! `DescribeDBProxyTargets` and `DeregisterDBProxyTargets`.
//!
//! Every call goes over the wire: the service is mounted on a `MultiServiceRouter` exactly
//! as the production router mounts it, and each request is a real awsQuery form POST
//! carrying the SigV4 credential scope the router keys on. The assertions read the XML the
//! emulator answers with, so a handler that computed nothing could not pass them.
//!
//! The rules under test, each asserted over a TABLE spanning the class it generalises over
//! rather than over one request:
//!
//! * a target reports the registration's own identity — `RdsResourceId` is "the identifier
//!   representing the target. It can be the instance identifier for an RDS DB instance, or
//!   the cluster identifier for an Aurora DB cluster", so it is the identifier the
//!   registration names, under the spelling the database is stored with, whatever
//!   server-assigned ids that database carries elsewhere — while its ARN, its endpoint and
//!   its port are read from that database's own record;
//! * a registered DB cluster is TRACKED: the proxy reaches the cluster's DB instances
//!   through it, so the report carries one `RDS_INSTANCE` target per member beside the
//!   cluster's own, computed from the cluster's current membership, the writer read/write
//!   and the readers read-only;
//! * a target's health walks the lifecycle `TargetHealth$State` publishes (`registering` >
//!   `unavailable` > `available`): a registration this service has not finished putting in
//!   force is `REGISTERING`, the clock alone settles it on `AVAILABLE`, and one whose
//!   database this service no longer holds is `UNAVAILABLE` for the reason `UNREACHABLE`;
//! * the report lists the registrations in registration order;
//! * registering a database the group already holds the registration for asks for exactly
//!   the registration that exists, so it is served with the target the group has and adds
//!   nothing, while a registration that would make the proxy reach a database it ALREADY
//!   reaches another way — whichever way round the two arrive, and in one call as well as
//!   in two — is the modelled `DBProxyTargetAlreadyRegisteredFault`;
//! * a deregistration of a database the proxy's target group does not hold is
//!   `DBProxyTargetNotFoundFault`, and it leaves the group exactly as it was.

use service_com_amazonaws_rds_amazonrdsv19::state::{
    DB_PROXY_TARGET_ALREADY_REGISTERED_MESSAGE, MISSING_DB_PROXY_TARGETS_MESSAGE,
    TARGET_ROLE_READ_ONLY, TARGET_ROLE_READ_WRITE, TARGET_STATE_AVAILABLE,
    TARGET_STATE_REGISTERING, TARGET_STATE_UNAVAILABLE, TARGET_TYPE_RDS_INSTANCE,
    TARGET_TYPE_TRACKED_CLUSTER,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, emulator, expect_error, expect_ok, optional_text, subtrees, text, texts,
    wait_for_settle, REGION,
};
use crate::db_proxy_target_groups::create_proxy;

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// `TargetHealthReason` — "`UNREACHABLE`", the reason a target whose database is gone
/// reports.
const TARGET_HEALTH_REASON_UNREACHABLE: &str = "UNREACHABLE";

/// One row of the registration table: the databases one `RegisterDBProxyTargets` call
/// names and what each of the targets it answers with must report.
struct RegistrationCase {
    case: &'static str,
    /// Whether the row names DB CLUSTERS rather than DB instances, which decides both the
    /// member the request carries them under and the `TargetType` each target reports.
    clusters: bool,
    /// The identifiers as the REQUEST spells them.
    requested: &'static [&'static str],
    /// The identifier each registration is stored and reported under: both database types'
    /// identifiers are "stored as a lowercase string", so a request that spells one
    /// differently registers the same database.
    reported: &'static [&'static str],
}

impl RegistrationCase {
    /// `TargetType` a registration of this row's kind of database reports.
    fn target_type(&self) -> &'static str {
        if self.clusters {
            TARGET_TYPE_TRACKED_CLUSTER
        } else {
            TARGET_TYPE_RDS_INSTANCE
        }
    }

    /// The `RegisterDBProxyTargets` / `DeregisterDBProxyTargets` member this row's
    /// identifiers are carried under, indexed from one.
    fn member(&self, index: usize) -> String {
        let member = if self.clusters {
            "DBClusterIdentifiers"
        } else {
            "DBInstanceIdentifiers"
        };
        format!("{member}.member.{}", index + 1)
    }
}

/// The rows differ along exactly what the rules generalise over: the KIND of database
/// registered (each `TargetType` a database this service holds can be), HOW MANY one call
/// names, and the SPELLING the request names them under.
const REGISTRATION_CASES: &[RegistrationCase] = &[
    RegistrationCase {
        case: "one DB instance",
        clusters: false,
        requested: &["emu-target-instance-a"],
        reported: &["emu-target-instance-a"],
    },
    RegistrationCase {
        case: "two DB instances in one call",
        clusters: false,
        requested: &["emu-target-instance-b", "emu-target-instance-c"],
        reported: &["emu-target-instance-b", "emu-target-instance-c"],
    },
    RegistrationCase {
        case: "a DB instance named in mixed case",
        clusters: false,
        requested: &["EMU-Target-Instance-D"],
        reported: &["emu-target-instance-d"],
    },
    RegistrationCase {
        case: "one DB cluster",
        clusters: true,
        requested: &["emu-target-cluster-a"],
        reported: &["emu-target-cluster-a"],
    },
    RegistrationCase {
        case: "two DB clusters in one call, the second named in mixed case",
        clusters: true,
        requested: &["emu-target-cluster-b", "EMU-Target-Cluster-C"],
        reported: &["emu-target-cluster-b", "emu-target-cluster-c"],
    },
];

/// `CreateDBInstance` with its three required members, waiting out the transition the
/// create owes so the instance is `available` when a registration names it.
async fn create_instance(router: &MultiServiceRouter, identifier: &str) {
    expect_ok(
        &format!("create instance {identifier}"),
        call(
            router,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", identifier),
                ("Engine", "mysql"),
                ("DBInstanceClass", "db.m5.large"),
            ],
        )
        .await,
    );
}

/// `CreateDBCluster` with its two required members.
async fn create_cluster(router: &MultiServiceRouter, identifier: &str) {
    expect_ok(
        &format!("create cluster {identifier}"),
        call(
            router,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", identifier),
                ("Engine", "aurora-mysql"),
            ],
        )
        .await,
    );
}

/// `CreateDBInstance` for an Aurora instance that joins `cluster`.
async fn create_member(router: &MultiServiceRouter, identifier: &str, cluster: &str) {
    expect_ok(
        &format!("create member {identifier}"),
        call(
            router,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", identifier),
                ("Engine", "aurora-mysql"),
                ("DBInstanceClass", "db.r5.large"),
                ("DBClusterIdentifier", cluster),
            ],
        )
        .await,
    );
}

/// The ARN Amazon RDS mints for a database of this kind, which is what the target behind
/// it reports as its `TargetArn`.
fn database_arn(identifier: &str, cluster: bool) -> String {
    let resource_type = if cluster { "cluster" } else { "db" };
    format!("arn:aws:rds:{REGION}:{ACCOUNT}:{resource_type}:{identifier}")
}

/// The endpoint the database itself publishes: a DB instance's `Endpoint.Address` and a DB
/// cluster's `Endpoint`, read through that database's own describe so the target cannot
/// report one the database does not.
async fn published_endpoint(router: &MultiServiceRouter, identifier: &str, cluster: bool) -> String {
    let (action, member, element) = if cluster {
        ("DescribeDBClusters", "DBClusterIdentifier", "Endpoint")
    } else {
        ("DescribeDBInstances", "DBInstanceIdentifier", "Address")
    };
    let body = expect_ok(
        &format!("describe {identifier}"),
        call(router, action, &[(member, identifier)]).await,
    );
    let found = texts(&body, element);
    assert_eq!(
        found.len(),
        1,
        "expected {identifier} to publish one {element}, got {body}"
    );
    found.into_iter().next().expect("checked just above")
}

/// `DescribeDBProxyTargets` for `proxy`, as the `<member>` subtree of each target it
/// reports, in the order it reports them.
async fn describe_targets(router: &MultiServiceRouter, case: &str, proxy: &str) -> Vec<String> {
    let body = expect_ok(
        case,
        call(
            router,
            "DescribeDBProxyTargets",
            &[("DBProxyName", proxy)],
        )
        .await,
    );
    subtrees(&body, "member")
}

/// Assert one `DBProxyTarget` subtree reports the registration of `identifier`, with
/// `state` as its health.
fn assert_target(
    case: &str,
    target: &str,
    identifier: &str,
    row: &RegistrationCase,
    state: &str,
    endpoint: Option<&str>,
) {
    for (element, expected) in [
        ("RdsResourceId", identifier.to_string()),
        ("Type", row.target_type().to_string()),
        ("Role", TARGET_ROLE_READ_WRITE.to_string()),
        ("TargetArn", database_arn(identifier, row.clusters)),
        ("State", state.to_string()),
    ] {
        let actual = text(target, element);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {element} {expected:?}, got {actual:?} in {target}"
        );
    }
    // "This field is blank when the target represents an RDS DB instance"
    // (`DBProxyTarget$TrackedClusterId`).
    assert_eq!(
        optional_text(target, "TrackedClusterId"),
        row.clusters.then(|| identifier.to_string()),
        "case {case:?}: only a tracked cluster names a cluster it tracks: {target}"
    );
    if let Some(endpoint) = endpoint {
        let actual = text(target, "Endpoint");
        assert_eq!(
            actual, endpoint,
            "case {case:?}: expected the endpoint the database itself publishes \
             ({endpoint:?}), got {actual:?} in {target}"
        );
    }
    assert!(
        !text(target, "Port").is_empty(),
        "case {case:?}: expected the port the proxy connects on, got {target}"
    );
}

#[tokio::test]
async fn every_registration_reports_its_own_identifier_and_settles_available_in_order() {
    let router = emulator();
    create_proxy(&router, "emu-target-proxy", "MYSQL").await;
    for row in REGISTRATION_CASES {
        for identifier in row.reported {
            if row.clusters {
                create_cluster(&router, identifier).await;
            } else {
                create_instance(&router, identifier).await;
            }
        }
    }
    // The proxy and every database come up on their own; only then does a registration
    // name databases this service holds available records of.
    wait_for_settle();

    // What each call ANSWERS with is the registration it just made, and a registration
    // that is not in force yet is still `registering`.
    let mut registration_order: Vec<String> = Vec::new();
    for row in REGISTRATION_CASES {
        let mut params = vec![("DBProxyName".to_string(), "emu-target-proxy".to_string())];
        for (index, identifier) in row.requested.iter().enumerate() {
            params.push((row.member(index), (*identifier).to_string()));
        }
        let body = expect_ok(row.case, call(&router, "RegisterDBProxyTargets", &params).await);
        let answered = subtrees(&body, "member");
        assert_eq!(
            answered.len(),
            row.reported.len(),
            "case {:?}: expected one target per database the call named, got {body}",
            row.case
        );
        for (target, identifier) in answered.iter().zip(row.reported) {
            assert_target(
                row.case,
                target,
                identifier,
                row,
                TARGET_STATE_REGISTERING,
                None,
            );
            // "If the State is AVAILABLE, a description is not included"
            // (`TargetHealth$Description`) — and a registration being put in force is
            // neither failing nor unreachable, so it reports no reason either.
            for element in ["Reason", "Description"] {
                assert_eq!(
                    optional_text(target, element),
                    None,
                    "case {:?}: a registration being put in force reports no {element}: \
                     {target}",
                    row.case
                );
            }
            registration_order.push((*identifier).to_string());
        }
    }

    // The clock alone moves a registration into force, and the report lists the
    // registrations in registration order.
    wait_for_settle();
    let reported = describe_targets(&router, "every registration", "emu-target-proxy").await;
    assert_eq!(
        reported.len(),
        registration_order.len(),
        "expected every registration to be reported, got {reported:?}"
    );
    let mut walked = reported.iter();
    for row in REGISTRATION_CASES {
        for identifier in row.reported {
            let target = walked
                .next()
                .unwrap_or_else(|| panic!("case {:?}: {identifier} is not reported", row.case));
            let endpoint = published_endpoint(&router, identifier, row.clusters).await;
            assert_target(
                row.case,
                target,
                identifier,
                row,
                TARGET_STATE_AVAILABLE,
                Some(&endpoint),
            );
        }
    }
    let order: Vec<String> = reported
        .iter()
        .map(|target| text(target, "RdsResourceId"))
        .collect();
    assert_eq!(
        order, registration_order,
        "expected the report to preserve registration order, got {order:?}"
    );

    // The report changed nothing a later caller sees.
    let again = describe_targets(&router, "the second report", "emu-target-proxy").await;
    assert_eq!(
        again, reported,
        "a report changes nothing a later caller sees: {reported:?} then {again:?}"
    );

    // Registering a database the group already holds a registration for asks for exactly
    // the registration that exists, so each row can be registered again: the call reports
    // the targets the group has — in force, not put back into `registering` — and adds
    // nothing, however many of its databases the request names.
    for row in REGISTRATION_CASES {
        let mut params = vec![("DBProxyName".to_string(), "emu-target-proxy".to_string())];
        for (index, identifier) in row.requested.iter().enumerate() {
            params.push((row.member(index), (*identifier).to_string()));
        }
        let body = expect_ok(
            &format!("{}, registered again", row.case),
            call(&router, "RegisterDBProxyTargets", &params).await,
        );
        let answered: Vec<String> = subtrees(&body, "member")
            .iter()
            .map(|target| text(target, "RdsResourceId"))
            .collect();
        assert_eq!(
            answered,
            row.reported
                .iter()
                .map(|identifier| (*identifier).to_string())
                .collect::<Vec<String>>(),
            "case {:?}: expected the targets the group already has, got {answered:?}",
            row.case
        );
        for target in subtrees(&body, "member") {
            assert_eq!(
                text(&target, "State"),
                TARGET_STATE_AVAILABLE,
                "case {:?}: a registration already in force stays in force: {target}",
                row.case
            );
        }
    }
    let unchanged = describe_targets(&router, "after the repeat registrations", "emu-target-proxy")
        .await;
    assert_eq!(
        unchanged, reported,
        "a registration that added nothing changed nothing: {reported:?} then {unchanged:?}"
    );
}

/// One row of the refusal table: a deregistration the proxy's target group cannot serve.
struct MissingTargetCase {
    case: &'static str,
    /// The identifiers the call names, each under the member for its kind of database.
    named: &'static [(&'static str, &'static str)],
}

/// The rows differ along exactly what the rule generalises over: WHY the group does not
/// hold the target the call names — a database it never had registered, one of the other
/// kind, one this service holds no record of at all, and a call that names a registered
/// database beside one of those.
const MISSING_TARGET_CASES: &[MissingTargetCase] = &[
    MissingTargetCase {
        case: "a DB instance that was never registered",
        named: &[("DBInstanceIdentifiers.member.1", "emu-dereg-spare")],
    },
    MissingTargetCase {
        case: "a DB cluster that was never registered",
        named: &[("DBClusterIdentifiers.member.1", "emu-dereg-cluster")],
    },
    MissingTargetCase {
        case: "a database this service holds no record of",
        named: &[("DBInstanceIdentifiers.member.1", "emu-dereg-nothing")],
    },
    MissingTargetCase {
        case: "the registered DB instance named as a DB cluster",
        named: &[("DBClusterIdentifiers.member.1", "emu-dereg-registered")],
    },
    MissingTargetCase {
        case: "the registered DB instance beside one that is not registered",
        named: &[
            ("DBInstanceIdentifiers.member.1", "emu-dereg-registered"),
            ("DBInstanceIdentifiers.member.2", "emu-dereg-spare"),
        ],
    },
];

#[tokio::test]
async fn deregistering_a_target_the_group_does_not_hold_is_refused_and_removes_nothing() {
    let router = emulator();
    create_proxy(&router, "emu-dereg-proxy", "MYSQL").await;
    create_instance(&router, "emu-dereg-registered").await;
    create_instance(&router, "emu-dereg-spare").await;
    create_cluster(&router, "emu-dereg-cluster").await;
    wait_for_settle();
    expect_ok(
        "the one registration",
        call(
            &router,
            "RegisterDBProxyTargets",
            &[
                ("DBProxyName", "emu-dereg-proxy"),
                ("DBInstanceIdentifiers.member.1", "emu-dereg-registered"),
            ],
        )
        .await,
    );
    wait_for_settle();

    for row in MISSING_TARGET_CASES {
        let mut params = vec![("DBProxyName", "emu-dereg-proxy")];
        params.extend_from_slice(row.named);
        let body = expect_error(
            row.case,
            call(&router, "DeregisterDBProxyTargets", &params).await,
            404,
            "DBProxyTargetNotFoundFault",
        );
        let message = text(&body, "Message");
        assert_eq!(
            message, MISSING_DB_PROXY_TARGETS_MESSAGE,
            "case {:?}: expected the fixed sentence about the missing targets, got \
             {message:?}",
            row.case
        );
        // Every refusal left the group exactly as it was — the row that named a
        // registered database beside an unregistered one included.
        let left = describe_targets(&router, row.case, "emu-dereg-proxy").await;
        assert_eq!(
            left.len(),
            1,
            "case {:?}: the refused deregistration removed nothing, got {left:?}",
            row.case
        );
        assert_eq!(
            text(&left[0], "RdsResourceId"),
            "emu-dereg-registered",
            "case {:?}: the one registration is still the group's, got {left:?}",
            row.case
        );
    }

    // The database the group DOES hold deregisters, and then it too is one the group does
    // not hold.
    expect_ok(
        "the deregistration",
        call(
            &router,
            "DeregisterDBProxyTargets",
            &[
                ("DBProxyName", "emu-dereg-proxy"),
                ("DBInstanceIdentifiers.member.1", "emu-dereg-registered"),
            ],
        )
        .await,
    );
    let left = describe_targets(&router, "after the deregistration", "emu-dereg-proxy").await;
    assert!(
        left.is_empty(),
        "expected the deregistered target to be gone, got {left:?}"
    );
    let body = expect_error(
        "the same deregistration again",
        call(
            &router,
            "DeregisterDBProxyTargets",
            &[
                ("DBProxyName", "emu-dereg-proxy"),
                ("DBInstanceIdentifiers.member.1", "emu-dereg-registered"),
            ],
        )
        .await,
        404,
        "DBProxyTargetNotFoundFault",
    );
    assert_eq!(
        text(&body, "Message"),
        MISSING_DB_PROXY_TARGETS_MESSAGE,
        "expected the fixed sentence about the missing targets, got {body}"
    );
}

/// One row of the tracked-cluster table: what the proxy's report must carry for a
/// registered DB cluster and for each DB instance the proxy reaches through it.
struct TrackedCase {
    case: &'static str,
    /// The identifier the target reports as its `RdsResourceId`.
    identifier: &'static str,
    target_type: &'static str,
    /// `Role`: the cluster's writer serves read/write operations and every reader
    /// read-only ones.
    role: &'static str,
}

/// The rows differ along exactly what the rule generalises over: WHICH database of a
/// tracked cluster the target reports — the cluster itself, its writer, and a reader —
/// each of which the proxy reaches through the one registration.
const TRACKED_CASES: &[TrackedCase] = &[
    TrackedCase {
        case: "the registered cluster itself",
        identifier: "emu-tracked-cluster",
        target_type: TARGET_TYPE_TRACKED_CLUSTER,
        role: TARGET_ROLE_READ_WRITE,
    },
    TrackedCase {
        case: "the cluster's writer",
        identifier: "emu-tracked-writer",
        target_type: TARGET_TYPE_RDS_INSTANCE,
        role: TARGET_ROLE_READ_WRITE,
    },
    TrackedCase {
        case: "the cluster's reader",
        identifier: "emu-tracked-reader",
        target_type: TARGET_TYPE_RDS_INSTANCE,
        role: TARGET_ROLE_READ_ONLY,
    },
];

#[tokio::test]
async fn a_registered_cluster_reports_the_instances_the_proxy_reaches_through_it() {
    let router = emulator();
    create_proxy(&router, "emu-tracked-proxy", "MYSQL").await;
    create_cluster(&router, "emu-tracked-cluster").await;
    // A cluster's first instance is its writer and every later one is a reader.
    create_member(&router, "emu-tracked-writer", "emu-tracked-cluster").await;
    create_member(&router, "emu-tracked-reader", "emu-tracked-cluster").await;
    wait_for_settle();
    expect_ok(
        "the cluster registration",
        call(
            &router,
            "RegisterDBProxyTargets",
            &[
                ("DBProxyName", "emu-tracked-proxy"),
                ("DBClusterIdentifiers.member.1", "emu-tracked-cluster"),
            ],
        )
        .await,
    );
    wait_for_settle();

    // The one registration is reported as the cluster's own target plus one target per DB
    // instance the proxy reaches through it, the writer ahead of the readers.
    let reported = describe_targets(&router, "the tracked cluster", "emu-tracked-proxy").await;
    assert_eq!(
        reported.len(),
        TRACKED_CASES.len(),
        "expected the cluster and each instance it tracks, got {reported:?}"
    );
    for (target, row) in reported.iter().zip(TRACKED_CASES) {
        for (element, expected) in [
            ("RdsResourceId", row.identifier.to_string()),
            ("Type", row.target_type.to_string()),
            ("Role", row.role.to_string()),
            (
                "TargetArn",
                database_arn(
                    row.identifier,
                    row.target_type == TARGET_TYPE_TRACKED_CLUSTER,
                ),
            ),
            ("State", TARGET_STATE_AVAILABLE.to_string()),
            // Every target of a tracked cluster names the cluster it is reached through.
            ("TrackedClusterId", "emu-tracked-cluster".to_string()),
        ] {
            let actual = text(target, element);
            assert_eq!(
                actual, expected,
                "case {:?}: expected {element} {expected:?}, got {actual:?} in {target}",
                row.case
            );
        }
        let endpoint = published_endpoint(
            &router,
            row.identifier,
            row.target_type == TARGET_TYPE_TRACKED_CLUSTER,
        )
        .await;
        assert_eq!(
            text(target, "Endpoint"),
            endpoint,
            "case {:?}: expected the endpoint that database publishes, got {target}",
            row.case
        );
    }

    // The membership is read from the cluster rather than frozen at registration: an
    // instance that joins afterwards is reached through the same registration, and one
    // that leaves is not reported any more.
    create_member(&router, "emu-tracked-joiner", "emu-tracked-cluster").await;
    wait_for_settle();
    let joined = describe_targets(&router, "after a member joined", "emu-tracked-proxy").await;
    let identifiers: Vec<String> = joined
        .iter()
        .map(|target| text(target, "RdsResourceId"))
        .collect();
    assert_eq!(
        identifiers,
        vec![
            "emu-tracked-cluster".to_string(),
            "emu-tracked-writer".to_string(),
            "emu-tracked-reader".to_string(),
            "emu-tracked-joiner".to_string(),
        ],
        "expected the new member to be reached through the registration too, got \
         {identifiers:?}"
    );
    // A DB instance the proxy ALREADY REACHES through the tracked cluster cannot be
    // registered on its own: the proxy is already associated with that database, by a
    // different registration than the one the request asks for.
    for (case, identifier) in [
        ("the cluster's writer", "emu-tracked-writer"),
        ("a member that joined later", "emu-tracked-joiner"),
    ] {
        let refused = expect_error(
            case,
            call(
                &router,
                "RegisterDBProxyTargets",
                &[
                    ("DBProxyName", "emu-tracked-proxy"),
                    ("DBInstanceIdentifiers.member.1", identifier),
                ],
            )
            .await,
            400,
            "DBProxyTargetAlreadyRegisteredFault",
        );
        assert_eq!(
            text(&refused, "Message"),
            DB_PROXY_TARGET_ALREADY_REGISTERED_MESSAGE,
            "case {case:?}: expected the fault's own statement about the association, got \
             {refused}"
        );
    }

    expect_ok(
        "delete the reader",
        call(
            &router,
            "DeleteDBInstance",
            &[
                ("DBInstanceIdentifier", "emu-tracked-reader"),
                ("SkipFinalSnapshot", "true"),
            ],
        )
        .await,
    );
    wait_for_settle();
    let left = describe_targets(&router, "after a member left", "emu-tracked-proxy").await;
    let identifiers: Vec<String> = left
        .iter()
        .map(|target| text(target, "RdsResourceId"))
        .collect();
    assert_eq!(
        identifiers,
        vec![
            "emu-tracked-cluster".to_string(),
            "emu-tracked-writer".to_string(),
            "emu-tracked-joiner".to_string(),
        ],
        "expected the instance that left the cluster to be gone from the report, got \
         {identifiers:?}"
    );

    // Deregistering the cluster takes every target the proxy reached through it.
    expect_ok(
        "deregister the cluster",
        call(
            &router,
            "DeregisterDBProxyTargets",
            &[
                ("DBProxyName", "emu-tracked-proxy"),
                ("DBClusterIdentifiers.member.1", "emu-tracked-cluster"),
            ],
        )
        .await,
    );
    let gone = describe_targets(&router, "after the deregistration", "emu-tracked-proxy").await;
    assert!(
        gone.is_empty(),
        "expected the cluster and the instances it tracked to be gone, got {gone:?}"
    );
}

#[tokio::test]
async fn a_database_the_proxy_already_reaches_cannot_be_registered_another_way() {
    let router = emulator();
    // One proxy per row, so each row starts from a group that holds nothing.
    for proxy in [
        "emu-reach-cluster-first",
        "emu-reach-instance-first",
        "emu-reach-one-call",
    ] {
        create_proxy(&router, proxy, "MYSQL").await;
    }
    create_cluster(&router, "emu-reach-cluster").await;
    create_member(&router, "emu-reach-member", "emu-reach-cluster").await;
    wait_for_settle();

    /// The two registrations that reach one and the same DB instance: the cluster that
    /// tracks it, and the instance itself.
    const CLUSTER: (&str, &str) = ("DBClusterIdentifiers.member.1", "emu-reach-cluster");
    const INSTANCE: (&str, &str) = ("DBInstanceIdentifiers.member.1", "emu-reach-member");

    // The rows differ along exactly what the rule generalises over: WHICH WAY ROUND the
    // two registrations arrive, and whether they arrive in one call or two.
    for (case, proxy, first, second) in [
        (
            "the cluster, then the instance it tracks",
            "emu-reach-cluster-first",
            Some(CLUSTER),
            INSTANCE,
        ),
        (
            "the instance, then the cluster that tracks it",
            "emu-reach-instance-first",
            Some(INSTANCE),
            CLUSTER,
        ),
        (
            "both in one call",
            "emu-reach-one-call",
            None,
            // The call names the instance beside the cluster that tracks it, so the
            // registrations it would make reach one database twice between themselves.
            INSTANCE,
        ),
    ] {
        let mut params = vec![("DBProxyName", proxy)];
        if let Some(first) = first {
            expect_ok(
                &format!("{case}: the first registration"),
                call(&router, "RegisterDBProxyTargets", &[("DBProxyName", proxy), first]).await,
            );
            params.push(second);
        } else {
            params.push(second);
            params.push(CLUSTER);
        }
        let refused = expect_error(
            case,
            call(&router, "RegisterDBProxyTargets", &params).await,
            400,
            "DBProxyTargetAlreadyRegisteredFault",
        );
        assert_eq!(
            text(&refused, "Message"),
            DB_PROXY_TARGET_ALREADY_REGISTERED_MESSAGE,
            "case {case:?}: expected the fault's own statement about the association, got \
             {refused}"
        );
        // The refused call registered nothing: the group holds what it held before it.
        let reported = describe_targets(&router, case, proxy).await;
        let identifiers: Vec<String> = reported
            .iter()
            .map(|target| text(target, "RdsResourceId"))
            .collect();
        let expected: Vec<String> = match first {
            Some(("DBClusterIdentifiers.member.1", cluster)) => vec![
                cluster.to_string(),
                "emu-reach-member".to_string(),
            ],
            Some((_, instance)) => vec![instance.to_string()],
            None => Vec::new(),
        };
        assert_eq!(
            identifiers, expected,
            "case {case:?}: the refused registration left the group as it was, got \
             {identifiers:?}"
        );
    }
}

#[tokio::test]
async fn a_registration_whose_database_is_gone_reports_an_unreachable_target() {
    let router = emulator();
    create_proxy(&router, "emu-gone-proxy", "MYSQL").await;
    create_instance(&router, "emu-gone-instance").await;
    create_cluster(&router, "emu-gone-cluster").await;
    wait_for_settle();
    expect_ok(
        "the registrations",
        call(
            &router,
            "RegisterDBProxyTargets",
            &[
                ("DBProxyName", "emu-gone-proxy"),
                ("DBInstanceIdentifiers.member.1", "emu-gone-instance"),
                ("DBClusterIdentifiers.member.1", "emu-gone-cluster"),
            ],
        )
        .await,
    );
    wait_for_settle();

    // Both databases go away under the registrations, which stand.
    for (action, member, identifier) in [
        (
            "DeleteDBInstance",
            "DBInstanceIdentifier",
            "emu-gone-instance",
        ),
        ("DeleteDBCluster", "DBClusterIdentifier", "emu-gone-cluster"),
    ] {
        expect_ok(
            &format!("delete {identifier}"),
            call(
                &router,
                action,
                &[(member, identifier), ("SkipFinalSnapshot", "true")],
            )
            .await,
        );
    }
    wait_for_settle();

    let reported = describe_targets(&router, "the targets left behind", "emu-gone-proxy").await;
    assert_eq!(
        reported.len(),
        2,
        "expected both registrations to stand, got {reported:?}"
    );
    for (target, identifier) in reported.iter().zip(["emu-gone-instance", "emu-gone-cluster"]) {
        assert_eq!(
            text(target, "RdsResourceId"),
            identifier,
            "expected the registration of {identifier} to stand, got {target}"
        );
        assert_eq!(
            text(target, "State"),
            TARGET_STATE_UNAVAILABLE,
            "case {identifier:?}: a target whose database is gone is not available: {target}"
        );
        assert_eq!(
            text(target, "Reason"),
            TARGET_HEALTH_REASON_UNREACHABLE,
            "case {identifier:?}: a target whose database is gone is unreachable: {target}"
        );
        assert_eq!(
            optional_text(target, "Endpoint"),
            None,
            "case {identifier:?}: a target whose database is gone reports no endpoint it \
             could be reached at: {target}"
        );
    }
}
