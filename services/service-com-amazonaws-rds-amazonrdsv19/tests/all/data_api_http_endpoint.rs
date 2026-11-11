// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the RDS Data API HTTP endpoint toggle: `EnableHttpEndpoint`,
//! `DisableHttpEndpoint` and the `DBCluster$HttpEndpointEnabled` both of them write.
//!
//! The rule under test: each toggle writes the Data API HTTP endpoint flag of the DB
//! cluster its `ResourceArn` names and reports that flag back with the cluster's ARN, so
//! every later read of the cluster — the other toggle, and `DescribeDBClusters` — reports
//! what the last toggle wrote; a `ResourceArn` that names no DB cluster of this account in
//! the request's Region is `ResourceNotFoundFault` (HTTP 404), a cluster the operation does
//! not apply to or one busy with another operation is `InvalidResourceStateFault` (HTTP
//! 400), and a value that is no Amazon RDS DB cluster ARN at all is refused on the member
//! that carried it — `smithy_query(query="operation", operation="EnableHttpEndpoint")` for
//! the two faults and their statuses.
//!
//! The tables span what the rule generalises over: both engines that run an Aurora cluster,
//! a cluster created with the endpoint already on and one left at its documented default,
//! an Aurora Serverless v2 cluster and a plain provisioned one, an identifier stored
//! lowercased from a mixed-case create, the toggle asked for twice in a row (the setting is
//! idempotent), each direction of the toggle, both the cluster types the operation does NOT
//! apply to (an Aurora Serverless v1 cluster and a non-Aurora Multi-AZ DB cluster, plus a
//! sibling control plane's engine), the statuses that make a cluster busy, and the forms a
//! `ResourceArn` can take that resolve to nothing (another Region, another account, another
//! resource type, no ARN at all).
//!
//! Every call goes over the wire through the harness in `db_instances.rs`, so the
//! assertions read the XML the emulator itself answered with.

use service_com_amazonaws_rds_amazonrdsv19::state::{
    db_cluster_not_available_for_modification_message, db_cluster_not_found_colon_message,
    DATA_API_ENDPOINT_CLUSTER_TYPE_MESSAGE, DB_CLUSTER_STATUS_AVAILABLE,
    DB_CLUSTER_STATUS_CREATING, DB_CLUSTER_STATUS_STOPPED,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, call_in, emulator, expect_error, expect_ok, subtrees, text, wait_for_settle, REGION,
};

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// A second Region, so the REGIONAL scope of the cluster a `ResourceArn` resolves to is
/// observable.
const OTHER_REGION: &str = "eu-west-1";

/// The `arn:<partition>:rds:<region>:<account>:cluster:<identifier>` form Amazon RDS gives a
/// DB cluster ("Constructing an ARN for Amazon RDS"), written out here rather than taken
/// from the code under test.
fn cluster_arn(region: &str, identifier: &str) -> String {
    format!("arn:aws:rds:{region}:{ACCOUNT}:cluster:{identifier}")
}

/// One cluster the toggle is driven against.
struct ToggleCase {
    case: &'static str,
    /// The `DBClusterIdentifier` the create names.
    identifier: &'static str,
    /// What that identifier is stored and named in the ARN as: "this parameter is stored as
    /// a lowercase string".
    stored_identifier: &'static str,
    engine: &'static str,
    /// The parameters the create carries beyond `DBClusterIdentifier` and `Engine`.
    extra: &'static [(&'static str, &'static str)],
    /// Whether the cluster serves the Data API when it is created — `EnableHttpEndpoint`
    /// where the create names it, and the member's documented default (`false`) otherwise.
    created_enabled: bool,
}

/// The clusters the toggle round trip runs against: both Aurora engines, an Aurora
/// Serverless v2 capacity range and a plain provisioned cluster, a create that already
/// turned the endpoint on and creates that left it at its default, and an identifier the
/// create names in mixed case.
const TOGGLE_CASES: &[ToggleCase] = &[
    ToggleCase {
        case: "an aurora-mysql cluster created with the Data API endpoint already on",
        identifier: "emu-dataapi-mysql-on",
        stored_identifier: "emu-dataapi-mysql-on",
        engine: "aurora-mysql",
        extra: &[("EnableHttpEndpoint", "true")],
        created_enabled: true,
    },
    ToggleCase {
        case: "an Aurora Serverless v2 aurora-postgresql cluster at its documented default",
        identifier: "emu-dataapi-pg-serverless",
        stored_identifier: "emu-dataapi-pg-serverless",
        engine: "aurora-postgresql",
        extra: &[
            ("ServerlessV2ScalingConfiguration.MinCapacity", "0.5"),
            ("ServerlessV2ScalingConfiguration.MaxCapacity", "2.0"),
        ],
        created_enabled: false,
    },
    ToggleCase {
        case: "an aurora-mysql cluster named in mixed case, stored lowercased",
        identifier: "EMU-DataApi-MySQL-Mixed",
        stored_identifier: "emu-dataapi-mysql-mixed",
        engine: "aurora-mysql",
        extra: &[],
        created_enabled: false,
    },
    ToggleCase {
        case: "an aurora-postgresql cluster with a master user and the endpoint on",
        identifier: "emu-dataapi-pg-master",
        stored_identifier: "emu-dataapi-pg-master",
        engine: "aurora-postgresql",
        extra: &[
            ("MasterUsername", "emuadmin"),
            ("MasterUserPassword", "a-strong-password"),
            ("DatabaseName", "testdb"),
            ("EnableHttpEndpoint", "true"),
        ],
        created_enabled: true,
    },
];

/// `CreateDBCluster` for one row.
async fn create_cluster(
    router: &MultiServiceRouter,
    identifier: &str,
    engine: &str,
    extra: &[(&str, &str)],
) -> (u16, String) {
    let mut params: Vec<(&str, &str)> = vec![
        ("DBClusterIdentifier", identifier),
        ("Engine", engine),
    ];
    params.extend(extra.iter().copied());
    call(router, "CreateDBCluster", &params).await
}

/// The one `DBCluster` subtree `DescribeDBClusters` reports for `identifier`.
async fn describe_cluster(router: &MultiServiceRouter, case: &str, identifier: &str) -> String {
    let body = expect_ok(
        case,
        call(
            router,
            "DescribeDBClusters",
            &[("DBClusterIdentifier", identifier)],
        )
        .await,
    );
    let found = subtrees(&body, "DBCluster");
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one DBCluster for {identifier}, got {} in {body}",
        found.len()
    );
    found.into_iter().next().expect("checked just above")
}

/// What `DescribeDBClusters` reports for the cluster's Data API endpoint.
async fn reported_endpoint(router: &MultiServiceRouter, case: &str, identifier: &str) -> String {
    let reported = describe_cluster(router, case, identifier).await;
    text(&reported, "HttpEndpointEnabled")
}

/// One accepted toggle: assert the response reports `expected` for the cluster `arn` names,
/// and that the cluster itself reports the same afterwards.
async fn toggle(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    identifier: &str,
    arn: &str,
    expected: bool,
) {
    let body = expect_ok(case, call(router, action, &[("ResourceArn", arn)]).await);
    let reported_arn = text(&body, "ResourceArn");
    assert_eq!(
        reported_arn, arn,
        "case {case:?}: expected {action} to report the cluster ARN {arn:?}, got \
         {reported_arn:?}"
    );
    let enabled = text(&body, "HttpEndpointEnabled");
    assert_eq!(
        enabled,
        expected.to_string(),
        "case {case:?}: expected {action} to report HttpEndpointEnabled {expected}, got \
         {enabled:?}"
    );
    let persisted = reported_endpoint(router, case, identifier).await;
    assert_eq!(
        persisted,
        expected.to_string(),
        "case {case:?}: expected the cluster to report HttpEndpointEnabled {expected} after \
         {action}, got {persisted:?}"
    );
}

#[tokio::test]
async fn each_toggle_writes_the_data_api_endpoint_and_the_cluster_reads_it_back() {
    let router = emulator();

    for row in TOGGLE_CASES {
        expect_ok(
            row.case,
            create_cluster(&router, row.identifier, row.engine, row.extra).await,
        );
    }
    wait_for_settle();

    for (index, row) in TOGGLE_CASES.iter().enumerate() {
        let case = row.case;
        let arn = cluster_arn(REGION, row.stored_identifier);

        // What the create left, which is what the first toggle changes from.
        let status = text(
            &describe_cluster(&router, case, row.stored_identifier).await,
            "Status",
        );
        assert_eq!(
            status, DB_CLUSTER_STATUS_AVAILABLE,
            "case {case:?}: expected the cluster to be {DB_CLUSTER_STATUS_AVAILABLE} before \
             the toggle, got {status:?}"
        );
        let created = reported_endpoint(&router, case, row.stored_identifier).await;
        assert_eq!(
            created,
            row.created_enabled.to_string(),
            "case {case:?}: expected the created cluster to report HttpEndpointEnabled {}, \
             got {created:?}",
            row.created_enabled
        );

        // Enabled, then enabled again: the setting is idempotent, so the second call is
        // taken and reports the same value rather than refusing.
        toggle(
            &router,
            case,
            "EnableHttpEndpoint",
            row.stored_identifier,
            &arn,
            true,
        )
        .await;
        toggle(
            &router,
            case,
            "EnableHttpEndpoint",
            row.stored_identifier,
            &arn,
            true,
        )
        .await;
        // And off again, twice, which is the same rule in the other direction.
        toggle(
            &router,
            case,
            "DisableHttpEndpoint",
            row.stored_identifier,
            &arn,
            false,
        )
        .await;
        toggle(
            &router,
            case,
            "DisableHttpEndpoint",
            row.stored_identifier,
            &arn,
            false,
        )
        .await;
        // Left ON, so the rows processed before this one can be checked for cross-talk.
        toggle(
            &router,
            case,
            "EnableHttpEndpoint",
            row.stored_identifier,
            &arn,
            true,
        )
        .await;

        // A toggle writes the cluster its ARN named and no other: every row already driven
        // still reports what its own last toggle wrote.
        for earlier in &TOGGLE_CASES[..index] {
            let untouched = reported_endpoint(&router, earlier.case, earlier.stored_identifier).await;
            assert_eq!(
                untouched, "true",
                "case {case:?}: expected the toggle to leave {} alone, which the row before \
                 it left enabled, got {untouched:?}",
                earlier.stored_identifier
            );
        }

        // The toggle needs no restart of the cluster's instances, so the cluster keeps the
        // status it had.
        let after = text(
            &describe_cluster(&router, case, row.stored_identifier).await,
            "Status",
        );
        assert_eq!(
            after, DB_CLUSTER_STATUS_AVAILABLE,
            "case {case:?}: expected the toggle to leave the cluster \
             {DB_CLUSTER_STATUS_AVAILABLE}, got {after:?}"
        );
    }
}

/// A `ResourceArn` that resolves to no DB cluster of this account in the request's Region.
struct MissCase {
    case: &'static str,
    /// The value the refused call carries.
    resource_arn: String,
}

#[tokio::test]
async fn a_resource_arn_that_names_no_cluster_in_the_region_is_not_found() {
    let router = emulator();

    // One cluster in each Region, so a miss is a miss because of WHERE the ARN points
    // rather than because the store is empty.
    expect_ok(
        "setup",
        create_cluster(&router, "emu-dataapi-here", "aurora-mysql", &[]).await,
    );
    expect_ok(
        "setup",
        call_in(
            &router,
            OTHER_REGION,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", "emu-dataapi-elsewhere"),
                ("Engine", "aurora-mysql"),
            ],
        )
        .await,
    );
    wait_for_settle();

    let misses = [
        MissCase {
            case: "a cluster that was never created",
            resource_arn: cluster_arn(REGION, "emu-dataapi-absent"),
        },
        MissCase {
            case: "an existing cluster's ARN with another Region in it",
            resource_arn: cluster_arn(OTHER_REGION, "emu-dataapi-here"),
        },
        MissCase {
            case: "a cluster that exists only in another Region",
            resource_arn: cluster_arn(OTHER_REGION, "emu-dataapi-elsewhere"),
        },
        MissCase {
            case: "an existing cluster's ARN under another account",
            resource_arn: format!("arn:aws:rds:{REGION}:210987654321:cluster:emu-dataapi-here"),
        },
        MissCase {
            case: "a cluster ARN in another partition",
            resource_arn: format!("arn:aws-us-gov:rds:{REGION}:{ACCOUNT}:cluster:emu-dataapi-here"),
        },
    ];

    for row in &misses {
        for action in ["EnableHttpEndpoint", "DisableHttpEndpoint"] {
            let case = row.case;
            let refused = expect_error(
                case,
                call(&router, action, &[("ResourceArn", row.resource_arn.as_str())]).await,
                404,
                "ResourceNotFoundFault",
            );
            let message = text(&refused, "Message");
            let expected = db_cluster_not_found_colon_message(&row.resource_arn);
            assert_eq!(
                message, expected,
                "case {case:?} on {action}: expected the wording {expected:?}, got {message:?}"
            );
        }
    }

    // The cluster that IS in the request's Region still serves the toggle, so the refusals
    // above are about the ARN and not about the operation.
    let arn = cluster_arn(REGION, "emu-dataapi-here");
    toggle(
        &router,
        "the cluster in the request's Region",
        "EnableHttpEndpoint",
        "emu-dataapi-here",
        &arn,
        true,
    )
    .await;
}

#[tokio::test]
async fn a_value_that_is_no_db_cluster_arn_is_refused_on_the_member() {
    let router = emulator();
    expect_ok(
        "setup",
        create_cluster(&router, "emu-dataapi-member", "aurora-mysql", &[]).await,
    );
    wait_for_settle();

    // Every form that carries no Amazon RDS DB cluster ARN: the bare identifier, an ARN of
    // another RDS resource type, an ARN of another service, and a value that is no ARN.
    let unusable = [
        "emu-dataapi-member".to_string(),
        format!("arn:aws:rds:{REGION}:{ACCOUNT}:db:emu-dataapi-member"),
        format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster-snapshot:emu-dataapi-member"),
        format!("arn:aws:ec2:{REGION}:{ACCOUNT}:instance:i-0123456789abcdef0"),
        "not-an-arn-at-all".to_string(),
    ];

    for value in &unusable {
        for action in ["EnableHttpEndpoint", "DisableHttpEndpoint"] {
            let refused = expect_error(
                value,
                call(&router, action, &[("ResourceArn", value.as_str())]).await,
                400,
                "InvalidParameterValue",
            );
            let message = text(&refused, "Message");
            let expected = format!("Invalid resource name: {value}");
            assert_eq!(
                message, expected,
                "case {value:?} on {action}: expected the wording {expected:?}, got {message:?}"
            );
        }
    }

    // Nothing the refused calls named reached the cluster.
    let reported = reported_endpoint(&router, "the named cluster", "emu-dataapi-member").await;
    assert_eq!(
        reported, "false",
        "expected the refused calls to leave the cluster's endpoint alone, got {reported:?}"
    );
}

/// A cluster the toggle does not apply to: the operation "applies only to Aurora Serverless
/// v2 and provisioned DB clusters".
struct IneligibleCase {
    case: &'static str,
    identifier: &'static str,
    engine: &'static str,
    extra: &'static [(&'static str, &'static str)],
    /// The `EngineMode` a `ModifyDBCluster` moves the cluster into before the toggle
    /// arrives, for a row whose ineligibility is the mode rather than the engine: an
    /// Aurora Serverless v1 cluster runs the `serverless` mode, which no available engine
    /// version of this catalogue publishes, so the mode is reached by modifying the cluster
    /// into it.
    into_engine_mode: Option<&'static str>,
}

/// The settings a Multi-AZ DB cluster requires, as `CreateDBCluster` documents them for
/// that cluster type ("This setting is required to create a Multi-AZ DB cluster").
const MULTI_AZ_SETTINGS: &[(&str, &str)] = &[
    ("AllocatedStorage", "100"),
    ("DBClusterInstanceClass", "db.m6gd.xlarge"),
    ("StorageType", "io1"),
    ("Iops", "1000"),
];

/// The cluster types with no Data API HTTP endpoint to toggle: an Aurora Serverless v1
/// cluster (the `serverless` engine mode, whose endpoint `ModifyDBCluster` owns), a
/// non-Aurora Multi-AZ DB cluster, and a sibling control plane's engine.
const INELIGIBLE_CASES: &[IneligibleCase] = &[
    IneligibleCase {
        case: "an Aurora Serverless v1 aurora-mysql cluster",
        identifier: "emu-dataapi-v1-mysql",
        engine: "aurora-mysql",
        extra: &[],
        into_engine_mode: Some("serverless"),
    },
    IneligibleCase {
        case: "an Aurora Serverless v1 aurora-postgresql cluster",
        identifier: "emu-dataapi-v1-pg",
        engine: "aurora-postgresql",
        extra: &[("EnableHttpEndpoint", "true")],
        into_engine_mode: Some("serverless"),
    },
    IneligibleCase {
        case: "a non-Aurora Multi-AZ DB cluster",
        identifier: "emu-dataapi-multiaz",
        engine: "mysql",
        extra: MULTI_AZ_SETTINGS,
        into_engine_mode: None,
    },
    IneligibleCase {
        case: "a DocumentDB cluster",
        identifier: "emu-dataapi-docdb",
        engine: "docdb",
        extra: &[
            ("MasterUsername", "emuadmin"),
            ("MasterUserPassword", "pw1-Secret"),
        ],
        into_engine_mode: None,
    },
];

#[tokio::test]
async fn a_cluster_the_operation_does_not_apply_to_is_refused_on_its_type() {
    let router = emulator();

    for row in INELIGIBLE_CASES {
        expect_ok(
            row.case,
            create_cluster(&router, row.identifier, row.engine, row.extra).await,
        );
    }
    wait_for_settle();
    for row in INELIGIBLE_CASES {
        if let Some(mode) = row.into_engine_mode {
            expect_ok(
                row.case,
                call(
                    &router,
                    "ModifyDBCluster",
                    &[
                        ("DBClusterIdentifier", row.identifier),
                        ("EngineMode", mode),
                    ],
                )
                .await,
            );
        }
    }
    // The engine-mode change is one the cluster's instances restart for, so the clusters
    // moved into `serverless` are available again only after the span it owes.
    wait_for_settle();

    for row in INELIGIBLE_CASES {
        let case = row.case;
        let arn = cluster_arn(REGION, row.identifier);
        // The cluster really is available, so the refusal below is its TYPE answering
        // rather than a status the toggle would have to wait out.
        let status = text(&describe_cluster(&router, case, row.identifier).await, "Status");
        assert_eq!(
            status, DB_CLUSTER_STATUS_AVAILABLE,
            "case {case:?}: expected the cluster to be {DB_CLUSTER_STATUS_AVAILABLE} when the \
             toggle arrives, got {status:?}"
        );
        let before = reported_endpoint(&router, case, row.identifier).await;
        for action in ["EnableHttpEndpoint", "DisableHttpEndpoint"] {
            let refused = expect_error(
                case,
                call(&router, action, &[("ResourceArn", arn.as_str())]).await,
                400,
                "InvalidResourceStateFault",
            );
            let message = text(&refused, "Message");
            assert_eq!(
                message, DATA_API_ENDPOINT_CLUSTER_TYPE_MESSAGE,
                "case {case:?} on {action}: expected the wording \
                 {DATA_API_ENDPOINT_CLUSTER_TYPE_MESSAGE:?}, got {message:?}"
            );
        }
        // The refusal wrote nothing: the cluster reports the endpoint it had.
        let reported = reported_endpoint(&router, case, row.identifier).await;
        assert_eq!(
            reported, before,
            "case {case:?}: expected the refused toggles to leave the endpoint at \
             {before:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_cluster_busy_with_another_operation_is_refused_until_it_is_available() {
    let router = emulator();

    // A cluster whose stop has finished, settled before the row below creates the one that
    // is still coming up.
    expect_ok(
        "a stopped cluster",
        create_cluster(&router, "emu-dataapi-stopped", "aurora-mysql", &[]).await,
    );
    wait_for_settle();
    expect_ok(
        "a stopped cluster",
        call(
            &router,
            "StopDBCluster",
            &[("DBClusterIdentifier", "emu-dataapi-stopped")],
        )
        .await,
    );
    wait_for_settle();

    // And one still inside the span its create owes, so the two blocking statuses are both
    // met without waiting between them.
    expect_ok(
        "a cluster still coming up",
        create_cluster(&router, "emu-dataapi-creating", "aurora-mysql", &[]).await,
    );

    let busy = [
        ("a cluster still coming up", "emu-dataapi-creating", DB_CLUSTER_STATUS_CREATING),
        ("a stopped cluster", "emu-dataapi-stopped", DB_CLUSTER_STATUS_STOPPED),
    ];
    for (case, identifier, status) in busy {
        let reported = text(
            &describe_cluster(&router, case, identifier).await,
            "Status",
        );
        assert_eq!(
            reported, status,
            "case {case:?}: expected the cluster to be {status} when the toggle arrives, got \
             {reported:?}"
        );
        let arn = cluster_arn(REGION, identifier);
        for action in ["EnableHttpEndpoint", "DisableHttpEndpoint"] {
            let refused = expect_error(
                case,
                call(&router, action, &[("ResourceArn", arn.as_str())]).await,
                400,
                "InvalidResourceStateFault",
            );
            let message = text(&refused, "Message");
            let expected = db_cluster_not_available_for_modification_message(status);
            assert_eq!(
                message, expected,
                "case {case:?} on {action}: expected the wording {expected:?}, got {message:?}"
            );
        }
        let reported = reported_endpoint(&router, case, identifier).await;
        assert_eq!(
            reported, "false",
            "case {case:?}: expected the refused toggle to write nothing, got {reported:?}"
        );
    }

    // The refusal was about the STATE: once the clock has settled the cluster that was
    // coming up, the same call is taken.
    wait_for_settle();
    let arn = cluster_arn(REGION, "emu-dataapi-creating");
    toggle(
        &router,
        "a cluster still coming up",
        "EnableHttpEndpoint",
        "emu-dataapi-creating",
        &arn,
        true,
    )
    .await;
}
