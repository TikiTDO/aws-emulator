// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the control-plane slices this change implemented:
//! `ModifyDBCluster`, `CopyDBClusterParameterGroup`, the blue/green deployment operations,
//! `CreateIntegration` / `DescribeIntegrations`, the tenant-database operations, the RDS
//! Proxy target operations, `DescribeDBClusterAutomatedBackups` and the two DB log file
//! operations.
//!
//! Every call goes over the wire: the service is mounted on a `MultiServiceRouter` exactly
//! as the production router mounts it (through `db_instances::emulator`, the one mounting
//! helper this test target has), and each request is a real awsQuery form POST carrying the
//! SigV4 credential scope the router keys on. The assertions read the XML the emulator
//! answers with, so a handler that computed nothing could not pass them.
//!
//! The rules under test, each asserted over a TABLE spanning the class it generalises over
//! rather than over one request:
//!
//! * a modification writes every setting it names onto the stored resource and leaves it
//!   `modifying` until the clock settles it, and a resource that is not in a state the
//!   operation can be performed in is refused with the modelled state fault — which is
//!   what makes two modifications of one cluster serialise;
//! * a copy carries the source's own values into the target, and a source that resolves to
//!   nothing is the modelled not-found fault — by name and by ARN;
//! * a reference to another resource is resolved in the store, and WHICH resource type the
//!   reference names decides which not-found fault answers a reference that resolves to
//!   nothing; a reference that is not of the form the parameter accepts at all is the
//!   `InvalidParameterValue` the Query API answers a bad value with, and a system-assigned
//!   identifier the service never assigned is that too;
//! * a syntax check on one member runs BEFORE another member is resolved, so the first
//!   refusal does not depend on what the second names;
//! * a report computes its list from the records the writes persisted, and a reporting
//!   operation changes nothing a later caller sees.

use service_com_amazonaws_rds_amazonrdsv19::state::{
    engine_log, BLUE_GREEN_IDENTIFIER_PREFIX, BLUE_GREEN_STATUS_AVAILABLE,
    BLUE_GREEN_STATUS_DELETING, BLUE_GREEN_STATUS_PROVISIONING,
    DB_CLUSTER_AUTOMATED_BACKUP_STATUS_ACTIVE, DB_CLUSTER_STATUS_AVAILABLE,
    DB_CLUSTER_STATUS_MODIFYING, DB_PROXY_STATUS_AVAILABLE,
    DB_PROXY_TARGET_ALREADY_REGISTERED_MESSAGE, DEFAULT_DB_PROXY_TARGET_GROUP,
    DEFAULT_IDLE_CLIENT_TIMEOUT, INTEGRATION_STATUS_ACTIVE, INVALID_TARGET_TYPE_MESSAGE,
    TENANT_DATABASE_STATUS_AVAILABLE, TENANT_DATABASE_STATUS_DELETING,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, emulator, expect_error, expect_ok, optional_text, subtrees, text, texts,
    wait_for_settle, REGION,
};

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// An IAM role ARN of the form `CreateDBProxy$RoleArn` takes.
const PROXY_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/rds-proxy";

/// A subnet id of the published form (`subnet-` plus eight hexadecimal characters).
const SUBNET_ID: &str = "subnet-0a1b2c3d";

/// An Amazon Redshift namespace ARN, the kind of target `CreateIntegration$TargetArn` takes.
const REDSHIFT_TARGET_ARN: &str = "arn:aws:redshift-serverless:us-west-2:000000000000:namespace/zero-etl";

/// An Oracle container-database engine: the configuration tenant databases live in
/// (`CreateDBInstance$MultiTenant` "only applies to RDS for Oracle container database (CDB)
/// engines").
const CDB_ENGINE: &str = "oracle-ee-cdb";

/// `CreateDBInstance` with its three required members plus `extra`, waiting out the
/// transition the create owes so the instance is `available` when the test uses it.
async fn create_available_instance(
    router: &MultiServiceRouter,
    identifier: &str,
    engine: &str,
    extra: &[(&str, &str)],
) {
    let mut params = vec![
        ("DBInstanceIdentifier", identifier),
        ("Engine", engine),
        ("DBInstanceClass", "db.m5.large"),
    ];
    params.extend_from_slice(extra);
    expect_ok(
        &format!("create instance {identifier}"),
        call(router, "CreateDBInstance", &params).await,
    );
    wait_for_settle();
}

/// `CreateDBCluster` with its two required members plus `extra`, waiting out the transition
/// the create owes so the cluster is `available` when the test uses it.
async fn create_available_cluster(
    router: &MultiServiceRouter,
    identifier: &str,
    extra: &[(&str, &str)],
) {
    let mut params = vec![
        ("DBClusterIdentifier", identifier),
        ("Engine", "aurora-mysql"),
    ];
    params.extend_from_slice(extra);
    expect_ok(
        &format!("create cluster {identifier}"),
        call(router, "CreateDBCluster", &params).await,
    );
    wait_for_settle();
}

/// The one `DBCluster` subtree `DescribeDBClusters` reports for `identifier`.
async fn describe_cluster(router: &MultiServiceRouter, identifier: &str) -> String {
    let body = expect_ok(
        &format!("describe cluster {identifier}"),
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
        "case \"describe cluster {identifier}\": expected exactly one DBCluster, got {} in \
         {body}",
        found.len()
    );
    found.into_iter().next().expect("checked just above")
}

// ---------------------------------------------------------------------------
// ModifyDBCluster
// ---------------------------------------------------------------------------

/// One row of the modification table: the settings one `ModifyDBCluster` names, and what
/// the cluster reports afterwards.
struct ModifyCase {
    case: &'static str,
    /// The parameters this modification carries beyond `DBClusterIdentifier`.
    params: &'static [(&'static str, &'static str)],
    /// The elements the cluster reports after the modification settles, as
    /// `(element, text)`.
    expected: &'static [(&'static str, &'static str)],
    /// Whether applying this modification takes the cluster out of service, which is what
    /// decides the `Status` the modification is acknowledged with: a change the cluster's
    /// instances must be restarted to serve reports `modifying` until the clock settles it,
    /// and one that needs no downtime leaves the cluster `available`.
    out_of_service: bool,
}

/// The modifications the round trip applies and reads back.
///
/// The rows differ along what the rule generalises over: both published bounds of the
/// numeric members `ModifyDBCluster` documents a range for, each boolean member in both
/// directions, a window string, a storage type the cluster type publishes, and the log
/// export list the request adds to — and, across all of them, whether the change takes the
/// cluster out of service while it is applied.
const MODIFY_CASES: &[ModifyCase] = &[
    ModifyCase {
        case: "BackupRetentionPeriod at its published minimum",
        params: &[("BackupRetentionPeriod", "1")],
        expected: &[("BackupRetentionPeriod", "1")],
        out_of_service: false,
    },
    ModifyCase {
        case: "BackupRetentionPeriod at its published maximum",
        params: &[("BackupRetentionPeriod", "35")],
        expected: &[("BackupRetentionPeriod", "35")],
        out_of_service: false,
    },
    ModifyCase {
        case: "Port at its published minimum",
        params: &[("Port", "1150")],
        // `Port` is not read back: a cluster whose engine this emulator serves publishes
        // the port its own engine listener answers on, which the core assigns, so the
        // member reports the reachable port rather than echoing the configured one. What
        // this row still observes is that the change is accepted and takes the cluster out
        // of service while it is applied.
        expected: &[],
        out_of_service: true,
    },
    ModifyCase {
        case: "Port at its published maximum",
        params: &[("Port", "65535")],
        expected: &[],
        out_of_service: true,
    },
    ModifyCase {
        case: "BacktrackWindow at its published maximum",
        params: &[("BacktrackWindow", "259200")],
        expected: &[("BacktrackWindow", "259200")],
        out_of_service: false,
    },
    ModifyCase {
        case: "the booleans turned on together",
        params: &[
            ("DeletionProtection", "true"),
            ("CopyTagsToSnapshot", "true"),
            ("EnableIAMDatabaseAuthentication", "true"),
            ("EnableHttpEndpoint", "true"),
        ],
        expected: &[
            ("DeletionProtection", "true"),
            ("CopyTagsToSnapshot", "true"),
            ("IAMDatabaseAuthenticationEnabled", "true"),
            ("HttpEndpointEnabled", "true"),
        ],
        out_of_service: false,
    },
    ModifyCase {
        case: "the booleans turned back off",
        params: &[
            ("DeletionProtection", "false"),
            ("CopyTagsToSnapshot", "false"),
            ("EnableIAMDatabaseAuthentication", "false"),
        ],
        expected: &[
            ("DeletionProtection", "false"),
            ("CopyTagsToSnapshot", "false"),
            ("IAMDatabaseAuthenticationEnabled", "false"),
        ],
        out_of_service: false,
    },
    ModifyCase {
        case: "the maintenance and backup windows",
        params: &[
            ("PreferredBackupWindow", "04:00-04:30"),
            ("PreferredMaintenanceWindow", "sun:05:00-sun:05:30"),
        ],
        expected: &[
            ("PreferredBackupWindow", "04:00-04:30"),
            ("PreferredMaintenanceWindow", "sun:05:00-sun:05:30"),
        ],
        out_of_service: false,
    },
    ModifyCase {
        case: "a storage type the Aurora cluster type publishes",
        params: &[("StorageType", "aurora-iopt1")],
        expected: &[("StorageType", "aurora-iopt1")],
        out_of_service: false,
    },
    ModifyCase {
        case: "a CloudWatch log export the request enables",
        params: &[(
            "CloudwatchLogsExportConfiguration.EnableLogTypes.member.1",
            "audit",
        )],
        // The list element wraps each value it carries in a `member` element, which is
        // what the reader below sees as the list's text.
        expected: &[("EnabledCloudwatchLogsExports", "<member>audit</member>")],
        out_of_service: false,
    },
];

/// The values each ranged member refuses, as `(case, member, value)`: one step past each
/// published bound.
const REFUSED_MODIFY_VALUES: &[(&str, &str, &str)] = &[
    ("one below the BackupRetentionPeriod minimum", "BackupRetentionPeriod", "0"),
    ("one above the BackupRetentionPeriod maximum", "BackupRetentionPeriod", "36"),
    ("one below the Port minimum", "Port", "1149"),
    ("one above the Port maximum", "Port", "65536"),
    ("one above the BacktrackWindow maximum", "BacktrackWindow", "259201"),
];

#[tokio::test]
async fn modify_db_cluster_writes_every_named_setting_onto_the_stored_cluster() {
    let router = emulator();
    create_available_cluster(&router, "emu-modify-cluster", &[]).await;
    // The address the cluster publishes before any modification: its own engine listener's,
    // which a `Port` modification does not move (the core owns the port the listener was
    // assigned, and the configured port the request names is not what is published).
    let published = {
        let reported = describe_cluster(&router, "emu-modify-cluster").await;
        (text(&reported, "Endpoint"), text(&reported, "Port"))
    };

    for ModifyCase {
        case,
        params,
        expected,
        out_of_service,
    } in MODIFY_CASES
    {
        let mut request = vec![("DBClusterIdentifier", "emu-modify-cluster")];
        request.extend_from_slice(params);
        let answered = expect_ok(case, call(&router, "ModifyDBCluster", &request).await);
        // The acknowledgement reports the cluster as this call FOUND it: the transition a
        // modification starts is not in effect while the answer is written, so the status it
        // carries is the one the cluster still had.
        let status = text(&answered, "Status");
        assert_eq!(
            status, DB_CLUSTER_STATUS_AVAILABLE,
            "case {case:?}: expected the acknowledgement to report the status the cluster \
             carried, got {status:?}"
        );
        // The transition is what the NEXT read reports: a modification the cluster's
        // instances must be restarted for leaves the cluster `modifying` until the clock
        // settles it back, and one that needs no downtime leaves it as it was.
        let expected_status = if *out_of_service {
            DB_CLUSTER_STATUS_MODIFYING
        } else {
            DB_CLUSTER_STATUS_AVAILABLE
        };
        let during = text(
            &describe_cluster(&router, "emu-modify-cluster").await,
            "Status",
        );
        assert_eq!(
            during, expected_status,
            "case {case:?}: expected the cluster to report {expected_status} while the \
             modification applies, got {during:?}"
        );
        wait_for_settle();

        let reported = describe_cluster(&router, "emu-modify-cluster").await;
        let settled = text(&reported, "Status");
        assert_eq!(
            settled, DB_CLUSTER_STATUS_AVAILABLE,
            "case {case:?}: expected the cluster to settle back to \
             {DB_CLUSTER_STATUS_AVAILABLE}, got {settled:?}"
        );
        for (element, wanted) in *expected {
            let found = texts(&reported, element);
            assert!(
                found.iter().any(|value| value == wanted),
                "case {case:?}: expected <{element}> to carry {wanted:?}, got {found:?} in \
                 {reported}"
            );
        }
        // Whatever the modification was, the cluster still publishes the address a client
        // dials it at: the engine listener it was created with is the one it keeps.
        assert_eq!(
            (text(&reported, "Endpoint"), text(&reported, "Port")),
            published,
            "case {case:?}: a modification must leave the cluster on the endpoint it \
             published, got {reported}"
        );
    }

    // Every value outside a published range is refused, and the refusal leaves the
    // cluster exactly as it was.
    let before = describe_cluster(&router, "emu-modify-cluster").await;
    for (case, member, value) in REFUSED_MODIFY_VALUES {
        expect_error(
            case,
            call(
                &router,
                "ModifyDBCluster",
                &[("DBClusterIdentifier", "emu-modify-cluster"), (member, value)],
            )
            .await,
            400,
            "InvalidParameterValue",
        );
    }
    let after = describe_cluster(&router, "emu-modify-cluster").await;
    assert_eq!(
        text(&after, "Port"),
        text(&before, "Port"),
        "a refused modification must leave the cluster as it was: {after}"
    );

    // A cluster this service holds no record of, and a cluster parameter group it holds
    // none of, are each the modelled not-found fault of their own resource type.
    expect_error(
        "an unknown cluster",
        call(
            &router,
            "ModifyDBCluster",
            &[
                ("DBClusterIdentifier", "emu-modify-nope"),
                ("BackupRetentionPeriod", "7"),
            ],
        )
        .await,
        404,
        "DBClusterNotFoundFault",
    );
    expect_error(
        "an unknown cluster parameter group",
        call(
            &router,
            "ModifyDBCluster",
            &[
                ("DBClusterIdentifier", "emu-modify-cluster"),
                ("DBClusterParameterGroupName", "emu-no-such-group"),
            ],
        )
        .await,
        404,
        "DBClusterParameterGroupNotFound",
    );
    // A group this service DOES hold is taken, and the cluster reports it.
    expect_ok(
        "create the group the modification moves the cluster to",
        call(
            &router,
            "CreateDBClusterParameterGroup",
            &[
                ("DBClusterParameterGroupName", "emu-modify-group"),
                ("DBParameterGroupFamily", "aurora-mysql8.0"),
                ("Description", "the group under test"),
            ],
        )
        .await,
    );
    expect_ok(
        "move the cluster to the group",
        call(
            &router,
            "ModifyDBCluster",
            &[
                ("DBClusterIdentifier", "emu-modify-cluster"),
                ("DBClusterParameterGroupName", "emu-modify-group"),
            ],
        )
        .await,
    );
    wait_for_settle();
    let reported = describe_cluster(&router, "emu-modify-cluster").await;
    assert_eq!(
        text(&reported, "DBClusterParameterGroup"),
        "emu-modify-group",
        "the cluster must report the group the modification moved it to: {reported}"
    );
}

#[tokio::test]
async fn two_modifications_of_one_cluster_serialise() {
    let router = emulator();
    create_available_cluster(&router, "emu-race-cluster", &[]).await;

    // The first modification takes the cluster out of service — the `Port` its instances
    // accept connections on is one the engine has to be restarted to serve — so it leaves
    // the cluster `modifying` from the next read on; a second one arriving before the clock
    // has settled it sees a state the operation cannot be performed in. One accepted, one
    // refused — whichever order the two arrive in. Each modification carries a second,
    // READ-BACK setting beside the port, because a cluster whose engine this emulator serves
    // publishes the port its own listener answers on rather than the configured one.
    let first = expect_ok(
        "the accepted modification",
        call(
            &router,
            "ModifyDBCluster",
            &[
                ("DBClusterIdentifier", "emu-race-cluster"),
                ("Port", "3307"),
                ("PreferredBackupWindow", "04:00-04:30"),
            ],
        )
        .await,
    );
    assert_eq!(
        text(&first, "Status"),
        DB_CLUSTER_STATUS_AVAILABLE,
        "case \"the accepted modification\": the acknowledgement reports the status the \
         cluster carried: {first}"
    );
    assert_eq!(
        text(&describe_cluster(&router, "emu-race-cluster").await, "Status"),
        DB_CLUSTER_STATUS_MODIFYING,
        "case \"the accepted modification\": the next read reports the cluster out of service"
    );
    for case in ["the first loser", "the second loser"] {
        expect_error(
            case,
            call(
                &router,
                "ModifyDBCluster",
                &[
                    ("DBClusterIdentifier", "emu-race-cluster"),
                    ("Port", "3308"),
                    ("PreferredBackupWindow", "05:00-05:30"),
                ],
            )
            .await,
            400,
            "InvalidDBClusterStateFault",
        );
    }
    // The accepted one is the one that took effect.
    wait_for_settle();
    let reported = describe_cluster(&router, "emu-race-cluster").await;
    assert_eq!(
        text(&reported, "PreferredBackupWindow"),
        "04:00-04:30",
        "the accepted modification is the one the cluster keeps: {reported}"
    );
    // Once the cluster is back to `available` a further modification is taken again.
    expect_ok(
        "after the cluster settles",
        call(
            &router,
            "ModifyDBCluster",
            &[
                ("DBClusterIdentifier", "emu-race-cluster"),
                ("Port", "3308"),
            ],
        )
        .await,
    );
}

// ---------------------------------------------------------------------------
// CopyDBClusterParameterGroup
// ---------------------------------------------------------------------------

#[tokio::test]
async fn copying_a_cluster_parameter_group_carries_the_sources_values() {
    let router = emulator();
    expect_ok(
        "create the source group",
        call(
            &router,
            "CreateDBClusterParameterGroup",
            &[
                ("DBClusterParameterGroupName", "emu-copy-source"),
                ("DBParameterGroupFamily", "docdb5.0"),
                ("Description", "the source group"),
            ],
        )
        .await,
    );
    expect_ok(
        "set a parameter on the source",
        call(
            &router,
            "ModifyDBClusterParameterGroup",
            &[
                ("DBClusterParameterGroupName", "emu-copy-source"),
                ("Parameters.Parameter.1.ParameterName", "tls"),
                ("Parameters.Parameter.1.ParameterValue", "disabled"),
                ("Parameters.Parameter.1.ApplyMethod", "pending-reboot"),
            ],
        )
        .await,
    );

    // Both forms `SourceDBClusterParameterGroupIdentifier` accepts — "the identifier or
    // Amazon Resource Name (ARN) for the source DB cluster parameter group" — name the
    // same source, and the copy carries the source's family and its parameter values.
    let source_arn = format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster-pg:emu-copy-source");
    for (case, named_by, target) in [
        ("by name", "emu-copy-source", "emu-copy-by-name"),
        ("by ARN", source_arn.as_str(), "emu-copy-by-arn"),
    ] {
        let copied = expect_ok(
            case,
            call(
                &router,
                "CopyDBClusterParameterGroup",
                &[
                    ("SourceDBClusterParameterGroupIdentifier", named_by),
                    ("TargetDBClusterParameterGroupIdentifier", target),
                    ("TargetDBClusterParameterGroupDescription", "the copy"),
                ],
            )
            .await,
        );
        assert_eq!(
            text(&copied, "DBParameterGroupFamily"),
            "docdb5.0",
            "case {case:?}: the copy carries the source's family: {copied}"
        );
        assert_eq!(
            text(&copied, "Description"),
            "the copy",
            "case {case:?}: the copy carries the description the request gave it: {copied}"
        );
        assert_eq!(
            text(&copied, "DBClusterParameterGroupArn"),
            format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster-pg:{target}"),
            "case {case:?}: the copy reports its own ARN: {copied}"
        );
        // The customised value is in the copy, read back through the describe that
        // reports a group's parameters.
        let parameters = expect_ok(
            case,
            call(
                &router,
                "DescribeDBClusterParameters",
                &[
                    ("DBClusterParameterGroupName", target),
                    ("Source", "user"),
                ],
            )
            .await,
        );
        assert!(
            texts(&parameters, "ParameterName").iter().any(|n| n == "tls")
                && texts(&parameters, "ParameterValue")
                    .iter()
                    .any(|v| v == "disabled"),
            "case {case:?}: the copy carries the source's customised parameter: {parameters}"
        );
    }

    // A source that resolves to nothing is the modelled not-found fault, in either form,
    // and a default group — one AWS provides per family, which "you can't copy" — is a
    // value the parameter does not accept.
    for (case, source) in [
        ("an unknown name", "emu-copy-nope"),
        (
            "an unknown ARN",
            "arn:aws:rds:us-west-2:000000000000:cluster-pg:emu-copy-nope",
        ),
    ] {
        expect_error(
            case,
            call(
                &router,
                "CopyDBClusterParameterGroup",
                &[
                    ("SourceDBClusterParameterGroupIdentifier", source),
                    ("TargetDBClusterParameterGroupIdentifier", "emu-copy-target"),
                    ("TargetDBClusterParameterGroupDescription", "the copy"),
                ],
            )
            .await,
            404,
            "DBParameterGroupNotFound",
        );
    }
    expect_error(
        "a default group",
        call(
            &router,
            "CopyDBClusterParameterGroup",
            &[
                (
                    "SourceDBClusterParameterGroupIdentifier",
                    "default.aurora-mysql8.0",
                ),
                ("TargetDBClusterParameterGroupIdentifier", "emu-copy-default"),
                ("TargetDBClusterParameterGroupDescription", "the copy"),
            ],
        )
        .await,
        400,
        "InvalidParameterValue",
    );
    // A target name already taken is the modelled already-exists fault.
    expect_error(
        "a target that already exists",
        call(
            &router,
            "CopyDBClusterParameterGroup",
            &[
                ("SourceDBClusterParameterGroupIdentifier", "emu-copy-source"),
                ("TargetDBClusterParameterGroupIdentifier", "emu-copy-by-name"),
                ("TargetDBClusterParameterGroupDescription", "the copy"),
            ],
        )
        .await,
        400,
        "DBParameterGroupAlreadyExists",
    );
}

// ---------------------------------------------------------------------------
// The blue/green deployment operations
// ---------------------------------------------------------------------------

/// The `Status` a blue/green deployment response reports.
///
/// A `BlueGreenDeployment` carries its own `Status` AND the `Status` of each switchover
/// detail, and the two must agree: a resource in a deployment that is still provisioning
/// is not yet ready to switch over. So this reads every one of them and asserts they do.
fn deployment_status(case: &str, xml: &str) -> String {
    let found = texts(xml, "Status");
    assert!(
        !found.is_empty(),
        "case {case:?}: expected a Status in {xml}"
    );
    assert!(
        found.iter().all(|status| *status == found[0]),
        "case {case:?}: the deployment's status and its switchover details' must agree, \
         got {found:?} in {xml}"
    );
    found.into_iter().next().expect("checked just above")
}

#[tokio::test]
async fn a_blue_green_deployment_clones_its_source_into_a_green_environment() {
    let router = emulator();
    create_available_instance(&router, "emu-bg-source", "mysql", &[]).await;
    let source_arn = format!("arn:aws:rds:{REGION}:{ACCOUNT}:db:emu-bg-source");

    let created = expect_ok(
        "create the deployment",
        call(
            &router,
            "CreateBlueGreenDeployment",
            &[
                ("BlueGreenDeploymentName", "emu-bg"),
                ("Source", source_arn.as_str()),
            ],
        )
        .await,
    );
    let identifier = text(&created, "BlueGreenDeploymentIdentifier");
    assert!(
        identifier.starts_with(BLUE_GREEN_IDENTIFIER_PREFIX),
        "the service assigns the identifier, which carries the \
         {BLUE_GREEN_IDENTIFIER_PREFIX} prefix: got {identifier:?}"
    );
    assert_eq!(
        deployment_status("create the deployment", &created),
        BLUE_GREEN_STATUS_PROVISIONING,
        "resources are being created in the green environment: {created}"
    );
    assert_eq!(
        text(&created, "Source"),
        source_arn,
        "the deployment reports its source: {created}"
    );
    let target_arn = text(&created, "Target");
    assert!(
        target_arn.contains("-green-") && target_arn.starts_with("arn:aws:rds:"),
        "the green environment is a database of this service, named after the blue one: \
         got {target_arn:?}"
    );
    let green = target_arn
        .rsplit(':')
        .next()
        .expect("a target ARN ends in the green database's identifier")
        .to_string();

    // The green database is a real DB instance record: a describe of it reports the clone.
    wait_for_settle();
    let green_reported = expect_ok(
        "describe the green instance",
        call(
            &router,
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", green.as_str())],
        )
        .await,
    );
    assert_eq!(
        text(&green_reported, "Engine"),
        "mysql",
        "the green database runs the blue one's engine: {green_reported}"
    );
    assert_eq!(
        text(&green_reported, "DBInstanceStatus"),
        "available",
        "the green database comes up on its own: {green_reported}"
    );

    // And the deployment itself is read back through its own describe, by identifier and
    // through each filter name the member documents.
    let reported = expect_ok(
        "describe the deployment",
        call(
            &router,
            "DescribeBlueGreenDeployments",
            &[("BlueGreenDeploymentIdentifier", identifier.as_str())],
        )
        .await,
    );
    assert_eq!(
        deployment_status("describe the deployment", &reported),
        BLUE_GREEN_STATUS_AVAILABLE,
        "the deployment reaches AVAILABLE on its own: {reported}"
    );
    assert_eq!(
        text(&reported, "SourceMember"),
        source_arn,
        "the switchover detail pairs the source with the target: {reported}"
    );
    for (case, filter, value) in [
        ("by identifier", "blue-green-deployment-identifier", identifier.as_str()),
        ("by name", "blue-green-deployment-name", "emu-bg"),
        ("by source", "source", source_arn.as_str()),
        ("by target", "target", target_arn.as_str()),
    ] {
        let filtered = expect_ok(
            case,
            call(
                &router,
                "DescribeBlueGreenDeployments",
                &[
                    ("Filters.Filter.1.Name", filter),
                    ("Filters.Filter.1.Values.Value.1", value),
                ],
            )
            .await,
        );
        assert_eq!(
            texts(&filtered, "BlueGreenDeploymentIdentifier"),
            vec![identifier.clone()],
            "case {case:?}: the filter reports the one deployment it names: {filtered}"
        );
    }
    let empty = expect_ok(
        "a filter value naming nothing",
        call(
            &router,
            "DescribeBlueGreenDeployments",
            &[
                ("Filters.Filter.1.Name", "blue-green-deployment-name"),
                ("Filters.Filter.1.Values.Value.1", "emu-bg-other"),
            ],
        )
        .await,
    );
    assert!(
        texts(&empty, "BlueGreenDeploymentIdentifier").is_empty(),
        "a filter that matches nothing reports nothing: {empty}"
    );
    expect_error(
        "an unsupported filter name",
        call(
            &router,
            "DescribeBlueGreenDeployments",
            &[
                ("Filters.Filter.1.Name", "deployment"),
                ("Filters.Filter.1.Values.Value.1", "emu-bg"),
            ],
        )
        .await,
        400,
        "InvalidParameterValue",
    );

    // A second deployment under the same name is the modelled already-exists fault.
    expect_error(
        "a name already taken",
        call(
            &router,
            "CreateBlueGreenDeployment",
            &[
                ("BlueGreenDeploymentName", "emu-bg"),
                ("Source", source_arn.as_str()),
            ],
        )
        .await,
        400,
        "BlueGreenDeploymentAlreadyExistsFault",
    );

    // The delete retires the deployment, and takes the green environment with it when
    // `DeleteTarget` asks for it.
    let deleted = expect_ok(
        "delete the deployment",
        call(
            &router,
            "DeleteBlueGreenDeployment",
            &[
                ("BlueGreenDeploymentIdentifier", identifier.as_str()),
                ("DeleteTarget", "true"),
            ],
        )
        .await,
    );
    assert_eq!(
        deployment_status("delete the deployment", &deleted),
        BLUE_GREEN_STATUS_DELETING,
        "the accepted delete reports the deployment DELETING: {deleted}"
    );
    let green_deleting = expect_ok(
        "the green instance after DeleteTarget",
        call(
            &router,
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", green.as_str())],
        )
        .await,
    );
    assert_eq!(
        text(&green_deleting, "DBInstanceStatus"),
        "deleting",
        "DeleteTarget deletes the resources in the green environment: {green_deleting}"
    );
    // A delete already running cannot be taken again, and once the record is gone every
    // read of the identifier is the modelled not-found fault.
    expect_error(
        "a delete already running",
        call(
            &router,
            "DeleteBlueGreenDeployment",
            &[("BlueGreenDeploymentIdentifier", identifier.as_str())],
        )
        .await,
        400,
        "InvalidBlueGreenDeploymentStateFault",
    );
    wait_for_settle();
    expect_error(
        "the deployment after it is gone",
        call(
            &router,
            "DescribeBlueGreenDeployments",
            &[("BlueGreenDeploymentIdentifier", identifier.as_str())],
        )
        .await,
        404,
        "BlueGreenDeploymentNotFoundFault",
    );
}

#[tokio::test]
async fn the_blue_green_operations_keep_their_two_validation_layers_apart() {
    let router = emulator();
    create_available_cluster(&router, "emu-bg-cluster", &[]).await;

    // `Source` is a database ARN: which resource type it names decides which not-found
    // fault answers one that resolves to nothing, and a value that is not the ARN of a DB
    // instance or DB cluster of this region is a value the parameter does not accept.
    for (case, source, status, code) in [
        (
            "a well-formed ARN naming a missing DB instance",
            format!("arn:aws:rds:{REGION}:{ACCOUNT}:db:emu-bg-missing"),
            404,
            "DBInstanceNotFound",
        ),
        (
            "a well-formed ARN naming a missing DB cluster",
            format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster:emu-bg-missing"),
            404,
            "DBClusterNotFoundFault",
        ),
        (
            "an ARN of another service",
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:bucket:emu-bg"),
            400,
            "InvalidParameterValue",
        ),
        (
            "an ARN of another RDS resource type",
            format!("arn:aws:rds:{REGION}:{ACCOUNT}:og:emu-bg"),
            400,
            "InvalidParameterValue",
        ),
        (
            "an ARN in another region",
            format!("arn:aws:rds:eu-west-1:{ACCOUNT}:db:emu-bg-source"),
            400,
            "InvalidParameterValue",
        ),
    ] {
        expect_error(
            case,
            call(
                &router,
                "CreateBlueGreenDeployment",
                &[
                    ("BlueGreenDeploymentName", "emu-bg-probe"),
                    ("Source", source.as_str()),
                ],
            )
            .await,
            status,
            code,
        );
    }

    // A `Source` ARN of ANOTHER region is refused by the sentence Amazon RDS words for it:
    // the ARN the request carried, single-quoted, and the kind of database that ARN names.
    for (case, source, database_kind) in [
        (
            "a DB instance ARN in another region",
            format!("arn:aws:rds:eu-west-1:{ACCOUNT}:db:emu-bg-source"),
            "DB Instance",
        ),
        (
            "a DB cluster ARN in another region",
            format!("arn:aws:rds:ap-south-1:{ACCOUNT}:cluster:emu-bg-cluster"),
            "DB Cluster",
        ),
    ] {
        let refused = expect_error(
            case,
            call(
                &router,
                "CreateBlueGreenDeployment",
                &[
                    ("BlueGreenDeploymentName", "emu-bg-probe"),
                    ("Source", source.as_str()),
                ],
            )
            .await,
            400,
            "InvalidParameterValue",
        );
        let message = text(&refused, "Message");
        assert_eq!(
            message,
            format!("'{source}' is not a valid {database_kind} ARN in this region"),
            "case {case:?}: got {message:?}"
        );
    }
    // A cluster source resolves, so the deployment is created from it.
    let created = expect_ok(
        "a cluster source",
        call(
            &router,
            "CreateBlueGreenDeployment",
            &[
                ("BlueGreenDeploymentName", "emu-bg-cluster-deployment"),
                (
                    "Source",
                    &format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster:emu-bg-cluster"),
                ),
            ],
        )
        .await,
    );
    assert!(
        text(&created, "Target").contains(":cluster:"),
        "a cluster source is cloned into a green CLUSTER: {created}"
    );

    // `BlueGreenDeploymentIdentifier` is system-assigned: an arbitrary one is a value the
    // parameter does not accept, while one of the assigned form that names no deployment
    // is the modelled not-found fault. Both layers hold for the delete and the describe.
    for action in ["DeleteBlueGreenDeployment", "DescribeBlueGreenDeployments"] {
        for (case, identifier, status, code) in [
            ("an arbitrary identifier", "emu-not-a-deployment", 400, "InvalidParameterValue"),
            (
                "a name without the assigned prefix",
                "bluegreen-0123456789abcdef",
                400,
                "InvalidParameterValue",
            ),
            (
                "an identifier of the assigned form that names nothing",
                "bgd-0123456789abcdef",
                404,
                "BlueGreenDeploymentNotFoundFault",
            ),
            (
                "an ARN carrying an identifier of the assigned form that names nothing",
                "arn:aws:rds:us-west-2:000000000000:blue-green-deployment:bgd-00000000deadbeef",
                404,
                "BlueGreenDeploymentNotFoundFault",
            ),
        ] {
            expect_error(
                &format!("{action}: {case}"),
                call(
                    &router,
                    action,
                    &[("BlueGreenDeploymentIdentifier", identifier)],
                )
                .await,
                status,
                code,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// CreateIntegration / DescribeIntegrations
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_integration_pairs_a_stored_database_with_a_target_warehouse() {
    let router = emulator();
    create_available_cluster(&router, "emu-zero-etl", &[]).await;
    let source_arn = format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster:emu-zero-etl");

    let created = expect_ok(
        "create the integration",
        call(
            &router,
            "CreateIntegration",
            &[
                ("IntegrationName", "emu-integration"),
                ("SourceArn", source_arn.as_str()),
                ("TargetArn", REDSHIFT_TARGET_ARN),
                ("Description", "the integration under test"),
            ],
        )
        .await,
    );
    let arn = text(&created, "IntegrationArn");
    assert!(
        arn.starts_with(&format!("arn:aws:rds:{REGION}:{ACCOUNT}:integration:")),
        "the integration reports its own ARN: got {arn:?}"
    );
    assert_eq!(
        text(&created, "Status"),
        "creating",
        "a new integration is not replicating yet: {created}"
    );

    // Read back through the describe, by name and by ARN — "the unique identifier of the
    // integration" in either form.
    wait_for_settle();
    for (case, identifier) in [
        ("by name", "emu-integration"),
        ("by ARN", arn.as_str()),
    ] {
        let reported = expect_ok(
            case,
            call(
                &router,
                "DescribeIntegrations",
                &[("IntegrationIdentifier", identifier)],
            )
            .await,
        );
        assert_eq!(
            text(&reported, "Status"),
            INTEGRATION_STATUS_ACTIVE,
            "case {case:?}: the integration becomes active on its own: {reported}"
        );
        assert_eq!(
            text(&reported, "SourceArn"),
            source_arn,
            "case {case:?}: the integration reports its source: {reported}"
        );
        assert_eq!(
            text(&reported, "TargetArn"),
            REDSHIFT_TARGET_ARN,
            "case {case:?}: the integration reports its target: {reported}"
        );
        assert_eq!(
            text(&reported, "Description"),
            "the integration under test",
            "case {case:?}: {reported}"
        );
    }
    expect_error(
        "an integration that does not exist",
        call(
            &router,
            "DescribeIntegrations",
            &[("IntegrationIdentifier", "emu-integration-nope")],
        )
        .await,
        404,
        "IntegrationNotFoundFault",
    );
    expect_error(
        "a name already taken",
        call(
            &router,
            "CreateIntegration",
            &[
                ("IntegrationName", "emu-integration"),
                ("SourceArn", source_arn.as_str()),
                ("TargetArn", REDSHIFT_TARGET_ARN),
            ],
        )
        .await,
        400,
        "IntegrationAlreadyExistsFault",
    );
}

#[tokio::test]
async fn create_integration_checks_the_target_before_it_resolves_the_source() {
    let router = emulator();

    // Both members name something that does not resolve: the answer is the TARGET's
    // refusal, because its syntax is checked before the source is looked up. That is the
    // order under test — the first refusal does not depend on what the source names.
    let refused = expect_error(
        "a malformed target and a missing source",
        call(
            &router,
            "CreateIntegration",
            &[
                ("IntegrationName", "emu-order-probe"),
                (
                    "SourceArn",
                    &format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster:emu-order-missing"),
                ),
                ("TargetArn", "not-a-valid-arn-but-long-enough"),
            ],
        )
        .await,
        400,
        "InvalidParameterValue",
    );
    assert_eq!(
        text(&refused, "Message"),
        INVALID_TARGET_TYPE_MESSAGE,
        "the refusal is the service's own for a target of the wrong kind: {refused}"
    );

    // With an acceptable target, the SOURCE is resolved, and which resource type its ARN
    // names decides the not-found fault.
    for (case, source, code) in [
        (
            "a missing DB cluster source",
            format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster:emu-order-missing"),
            "DBClusterNotFoundFault",
        ),
        (
            "a missing DB instance source",
            format!("arn:aws:rds:{REGION}:{ACCOUNT}:db:emu-order-missing"),
            "DBInstanceNotFound",
        ),
    ] {
        expect_error(
            case,
            call(
                &router,
                "CreateIntegration",
                &[
                    ("IntegrationName", "emu-order-probe"),
                    ("SourceArn", source.as_str()),
                    ("TargetArn", REDSHIFT_TARGET_ARN),
                ],
            )
            .await,
            404,
            code,
        );
    }

    // The target kinds the refusal names are accepted; a target of any other kind is
    // refused with that refusal, however long and however ARN-shaped it is.
    create_available_cluster(&router, "emu-target-kinds", &[]).await;
    let source_arn = format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster:emu-target-kinds");
    let target_kinds: [(&str, String, bool); 5] = [
        (
            "a Redshift provisioned cluster",
            format!("arn:aws:redshift:{REGION}:{ACCOUNT}:namespace/warehouse-1"),
            true,
        ),
        (
            "a Redshift Serverless namespace",
            REDSHIFT_TARGET_ARN.to_string(),
            true,
        ),
        (
            "an AWS Glue Data Catalog",
            format!("arn:aws:glue:{REGION}:{ACCOUNT}:catalog"),
            true,
        ),
        (
            "an Amazon S3 bucket",
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:bucket/warehouse"),
            false,
        ),
        (
            "an ARN with no resource part",
            format!("arn:aws:redshift:{REGION}:{ACCOUNT}:"),
            false,
        ),
    ];
    for (row, (case, target, accepted)) in target_kinds.iter().enumerate() {
        // One integration name per row: a name already taken is a different refusal.
        let name = format!("emu-target-kind-{row}");
        let response = call(
            &router,
            "CreateIntegration",
            &[
                ("IntegrationName", name.as_str()),
                ("SourceArn", source_arn.as_str()),
                ("TargetArn", target.as_str()),
            ],
        )
        .await;
        if *accepted {
            let created = expect_ok(case, response);
            assert_eq!(
                text(&created, "TargetArn"),
                *target,
                "case {case:?}: the integration reports the target it was given: {created}"
            );
        } else {
            let refused = expect_error(case, response, 400, "InvalidParameterValue");
            assert_eq!(
                text(&refused, "Message"),
                INVALID_TARGET_TYPE_MESSAGE,
                "case {case:?}: {refused}"
            );
        }
    }

    // A KMS key identifier of no recognisable form names no key, which is the modelled
    // fault for a key this service cannot reach.
    expect_error(
        "a KMS key that is not a key identifier",
        call(
            &router,
            "CreateIntegration",
            &[
                ("IntegrationName", "emu-kms-probe"),
                ("SourceArn", source_arn.as_str()),
                ("TargetArn", REDSHIFT_TARGET_ARN),
                ("KMSKeyId", "not-a-key"),
            ],
        )
        .await,
        400,
        "KMSKeyNotAccessibleFault",
    );
}

// ---------------------------------------------------------------------------
// The tenant-database operations
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_tenant_database_lifecycle_is_computed_from_persisted_state() {
    let router = emulator();
    create_available_instance(&router, "emu-cdb", CDB_ENGINE, &[]).await;

    let created = expect_ok(
        "create the tenant database",
        call(
            &router,
            "CreateTenantDatabase",
            &[
                ("DBInstanceIdentifier", "emu-cdb"),
                ("TenantDBName", "tenant1"),
                ("MasterUsername", "tenantadmin"),
                ("MasterUserPassword", "a-secret-password"),
            ],
        )
        .await,
    );
    assert_eq!(
        text(&created, "Status"),
        "creating",
        "a new tenant database is not usable yet: {created}"
    );
    let resource_id = text(&created, "TenantDatabaseResourceId");
    assert_eq!(
        text(&created, "TenantDatabaseARN"),
        format!("arn:aws:rds:{REGION}:{ACCOUNT}:tenant-database:{resource_id}"),
        "the tenant database reports its own ARN: {created}"
    );
    assert!(
        optional_text(&created, "MasterUserPassword").is_none(),
        "the master password is never reported back: {created}"
    );

    // Read back through the describe: by instance, by tenant name, and through each
    // filter name the member documents.
    wait_for_settle();
    let reported = expect_ok(
        "describe the instance's tenant databases",
        call(
            &router,
            "DescribeTenantDatabases",
            &[("DBInstanceIdentifier", "emu-cdb")],
        )
        .await,
    );
    assert_eq!(
        text(&reported, "TenantDBName"),
        "tenant1",
        "the describe reports what the create persisted: {reported}"
    );
    assert_eq!(
        text(&reported, "Status"),
        TENANT_DATABASE_STATUS_AVAILABLE,
        "the tenant database becomes available on its own: {reported}"
    );
    assert_eq!(
        text(&reported, "MasterUsername"),
        "tenantadmin",
        "{reported}"
    );
    let dbi_resource_id = text(&reported, "DbiResourceId");
    for (case, filter, value) in [
        ("by tenant name", "tenant-db-name", "tenant1"),
        (
            "by tenant resource id",
            "tenant-database-resource-id",
            resource_id.as_str(),
        ),
        ("by instance resource id", "dbi-resource-id", dbi_resource_id.as_str()),
    ] {
        let filtered = expect_ok(
            case,
            call(
                &router,
                "DescribeTenantDatabases",
                &[
                    ("Filters.Filter.1.Name", filter),
                    ("Filters.Filter.1.Values.Value.1", value),
                ],
            )
            .await,
        );
        assert_eq!(
            texts(&filtered, "TenantDBName"),
            vec!["tenant1".to_string()],
            "case {case:?}: the filter reports the one tenant database it names: {filtered}"
        );
    }
    expect_error(
        "a second tenant database of the same name",
        call(
            &router,
            "CreateTenantDatabase",
            &[
                ("DBInstanceIdentifier", "emu-cdb"),
                ("TenantDBName", "tenant1"),
                ("MasterUsername", "tenantadmin"),
            ],
        )
        .await,
        400,
        "TenantDatabaseAlreadyExists",
    );

    // The delete takes a final DB snapshot of the instance unless it is skipped, and the
    // snapshot it took is reported by the snapshot describe.
    let deleted = expect_ok(
        "delete the tenant database",
        call(
            &router,
            "DeleteTenantDatabase",
            &[
                ("DBInstanceIdentifier", "emu-cdb"),
                ("TenantDBName", "tenant1"),
                ("FinalDBSnapshotIdentifier", "emu-cdb-final"),
            ],
        )
        .await,
    );
    assert_eq!(
        text(&deleted, "Status"),
        TENANT_DATABASE_STATUS_DELETING,
        "the accepted delete reports the tenant database deleting: {deleted}"
    );
    let snapshots = expect_ok(
        "the final snapshot",
        call(
            &router,
            "DescribeDBSnapshots",
            &[("DBSnapshotIdentifier", "emu-cdb-final")],
        )
        .await,
    );
    assert_eq!(
        text(&snapshots, "DBInstanceIdentifier"),
        "emu-cdb",
        "the final snapshot is of the instance the tenant database lived in: {snapshots}"
    );
    wait_for_settle();
    let gone = expect_ok(
        "the instance's tenant databases after the delete",
        call(
            &router,
            "DescribeTenantDatabases",
            &[("DBInstanceIdentifier", "emu-cdb")],
        )
        .await,
    );
    assert!(
        texts(&gone, "TenantDBName").is_empty(),
        "the deleted tenant database leaves the service: {gone}"
    );
    expect_error(
        "a tenant database that is gone",
        call(
            &router,
            "DeleteTenantDatabase",
            &[
                ("DBInstanceIdentifier", "emu-cdb"),
                ("TenantDBName", "tenant1"),
                ("SkipFinalSnapshot", "true"),
            ],
        )
        .await,
        404,
        "TenantDatabaseNotFound",
    );
}

#[tokio::test]
async fn the_tenant_database_operations_resolve_the_instance_first() {
    let router = emulator();
    create_available_instance(&router, "emu-single-tenant", "mysql", &[]).await;
    create_available_instance(&router, "emu-multi-tenant", CDB_ENGINE, &[]).await;

    // An identifier that names no DB instance is the modelled not-found fault for every
    // tenant-database operation, whatever else the request carries — including the
    // final-snapshot parameters the delete would otherwise refuse.
    for (case, action, extra) in [
        ("create", "CreateTenantDatabase", &[("MasterUsername", "tenantadmin")][..]),
        ("describe", "DescribeTenantDatabases", &[][..]),
        ("delete", "DeleteTenantDatabase", &[][..]),
    ] {
        let mut params = vec![
            ("DBInstanceIdentifier", "emu-tenant-missing"),
            ("TenantDBName", "tenant1"),
        ];
        params.extend_from_slice(extra);
        let refused = expect_error(
            &format!("{case}: an instance that does not exist"),
            call(&router, action, &params).await,
            404,
            "DBInstanceNotFound",
        );
        assert!(
            text(&refused, "Message").contains("emu-tenant-missing"),
            "case {case:?}: the refusal names the instance the request asked for: {refused}"
        );
    }

    // An instance that exists but does not use the multi-tenant configuration holds no
    // tenant databases at all: that is a combination this service cannot resolve, not a
    // missing resource.
    for (case, action, extra) in [
        ("create", "CreateTenantDatabase", &[("MasterUsername", "tenantadmin")][..]),
        ("delete", "DeleteTenantDatabase", &[("SkipFinalSnapshot", "true")][..]),
    ] {
        let mut params = vec![
            ("DBInstanceIdentifier", "emu-single-tenant"),
            ("TenantDBName", "tenant1"),
        ];
        params.extend_from_slice(extra);
        expect_error(
            &format!("{case}: an instance that is not multi-tenant"),
            call(&router, action, &params).await,
            400,
            "InvalidParameterCombination",
        );
    }

    // On a multi-tenant instance, the tenant name itself is what resolves — or does not.
    expect_error(
        "a tenant database that does not exist",
        call(
            &router,
            "DeleteTenantDatabase",
            &[
                ("DBInstanceIdentifier", "emu-multi-tenant"),
                ("TenantDBName", "tenant-nope"),
                ("SkipFinalSnapshot", "true"),
            ],
        )
        .await,
        404,
        "TenantDatabaseNotFound",
    );
    // And the final-snapshot pair must hold together: neither both nor neither.
    expect_ok(
        "create a tenant database to delete",
        call(
            &router,
            "CreateTenantDatabase",
            &[
                ("DBInstanceIdentifier", "emu-multi-tenant"),
                ("TenantDBName", "tenant1"),
                ("MasterUsername", "tenantadmin"),
            ],
        )
        .await,
    );
    wait_for_settle();
    for (case, extra) in [
        ("neither", &[][..]),
        (
            "both",
            &[
                ("SkipFinalSnapshot", "true"),
                ("FinalDBSnapshotIdentifier", "emu-tenant-final"),
            ][..],
        ),
    ] {
        let mut params = vec![
            ("DBInstanceIdentifier", "emu-multi-tenant"),
            ("TenantDBName", "tenant1"),
        ];
        params.extend_from_slice(extra);
        expect_error(
            &format!("the final-snapshot pair: {case}"),
            call(&router, "DeleteTenantDatabase", &params).await,
            400,
            "InvalidParameterCombination",
        );
    }
}

// ---------------------------------------------------------------------------
// The RDS Proxy target operations
// ---------------------------------------------------------------------------

#[tokio::test]
async fn proxy_targets_are_registered_reported_and_deregistered() {
    let router = emulator();
    create_available_instance(&router, "emu-proxy-instance", "mysql", &[]).await;
    create_available_cluster(&router, "emu-proxy-cluster", &[]).await;

    let created = expect_ok(
        "create the proxy",
        call(
            &router,
            "CreateDBProxy",
            &[
                ("DBProxyName", "emu-proxy"),
                ("EngineFamily", "MYSQL"),
                ("RoleArn", PROXY_ROLE_ARN),
                ("VpcSubnetIds.member.1", SUBNET_ID),
                ("Auth.member.1.UserName", "appuser"),
                ("Auth.member.1.AuthScheme", "SECRETS"),
            ],
        )
        .await,
    );
    assert_eq!(
        text(&created, "Status"),
        "creating",
        "a new proxy is not ready to handle requests yet: {created}"
    );
    assert_eq!(
        text(&created, "DBProxyArn"),
        format!("arn:aws:rds:{REGION}:{ACCOUNT}:db-proxy:emu-proxy"),
        "the proxy reports its own ARN: {created}"
    );
    assert_eq!(
        text(&created, "IdleClientTimeout"),
        DEFAULT_IDLE_CLIENT_TIMEOUT.to_string(),
        "IdleClientTimeout takes its documented default: {created}"
    );

    // A proxy that is still coming up cannot take a registration; one that is up can.
    expect_error(
        "a registration before the proxy is available",
        call(
            &router,
            "RegisterDBProxyTargets",
            &[
                ("DBProxyName", "emu-proxy"),
                ("DBInstanceIdentifiers.member.1", "emu-proxy-instance"),
            ],
        )
        .await,
        400,
        "InvalidDBProxyStateFault",
    );
    wait_for_settle();
    let reported = expect_ok(
        "describe the proxy",
        call(&router, "DescribeDBProxies", &[("DBProxyName", "emu-proxy")]).await,
    );
    assert_eq!(
        text(&reported, "Status"),
        DB_PROXY_STATUS_AVAILABLE,
        "the proxy becomes available on its own: {reported}"
    );
    assert!(
        text(&reported, "Endpoint").starts_with("emu-proxy.proxy-"),
        "the proxy reports the endpoint a client connects to: {reported}"
    );

    // Each database type registers, and each reports what its own record says.
    for (case, member, identifier, target_type) in [
        (
            "a DB instance",
            "DBInstanceIdentifiers.member.1",
            "emu-proxy-instance",
            "RDS_INSTANCE",
        ),
        (
            "a DB cluster",
            "DBClusterIdentifiers.member.1",
            "emu-proxy-cluster",
            "TRACKED_CLUSTER",
        ),
    ] {
        let registered = expect_ok(
            case,
            call(
                &router,
                "RegisterDBProxyTargets",
                &[("DBProxyName", "emu-proxy"), (member, identifier)],
            )
            .await,
        );
        assert_eq!(
            text(&registered, "Type"),
            target_type,
            "case {case:?}: the target reports the kind of database it is: {registered}"
        );
        assert_eq!(
            text(&registered, "RdsResourceId"),
            identifier,
            "case {case:?}: the target reports the identifier it was registered under: \
             {registered}"
        );
        // A registration this service has not finished putting in force is still
        // `registering`, the state the target's health lifecycle starts in.
        assert_eq!(
            text(&registered, "State"),
            "REGISTERING",
            "case {case:?}: a just-made registration is still being put in force: \
             {registered}"
        );
        // Registering the same database again asks for the registration the group already
        // holds, so the call reports the target the group has for it and adds nothing —
        // the recorded behaviour of a client that registers one DB instance twice. The
        // registration the proxy CANNOT take is asserted at the end of this test.
        let again = expect_ok(
            &format!("{case}, again"),
            call(
                &router,
                "RegisterDBProxyTargets",
                &[("DBProxyName", "emu-proxy"), (member, identifier)],
            )
            .await,
        );
        for element in ["RdsResourceId", "Type", "TargetArn"] {
            assert_eq!(
                texts(&again, element),
                texts(&registered, element),
                "case {case:?}: registering the same database again reports the target the \
                 group already has, by {element}: {again}"
            );
        }
    }

    // The describe reports both registrations, each projected from its database, and each
    // registration is in force a moment after it was made.
    wait_for_settle();
    let targets = expect_ok(
        "describe the proxy's targets",
        call(
            &router,
            "DescribeDBProxyTargets",
            &[("DBProxyName", "emu-proxy")],
        )
        .await,
    );
    let mut resource_ids = texts(&targets, "RdsResourceId");
    resource_ids.sort();
    assert_eq!(
        resource_ids.len(),
        2,
        "both registrations are reported: {targets}"
    );
    assert_eq!(
        texts(&targets, "TrackedClusterId"),
        vec!["emu-proxy-cluster".to_string()],
        "only the cluster target reports a tracked cluster: {targets}"
    );
    assert_eq!(
        texts(&targets, "State"),
        vec!["AVAILABLE".to_string(), "AVAILABLE".to_string()],
        "a target whose database this service holds is available: {targets}"
    );
    // Each target reports the endpoint of the database behind it, which is the endpoint
    // that database itself publishes: a DB instance target reports exactly the
    // `Endpoint.Address` the instance's own describe reports, and a tracked cluster target
    // exactly the `Endpoint` the cluster's own describe reports — for both of them the
    // host of the engine listener this emulator serves them on (see `state::data_plane`).
    let instance = expect_ok(
        "the registered instance",
        call(
            &router,
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", "emu-proxy-instance")],
        )
        .await,
    );
    let published = text(&instance, "Address");
    let endpoints = texts(&targets, "Endpoint");
    assert!(
        endpoints.contains(&published),
        "the DB instance target must report the endpoint that instance publishes \
         ({published:?}): {targets}"
    );
    let cluster_endpoint = text(
        &describe_cluster(&router, "emu-proxy-cluster").await,
        "Endpoint",
    );
    assert!(
        endpoints.contains(&cluster_endpoint),
        "the tracked cluster target must report the endpoint that cluster publishes \
         ({cluster_endpoint:?}): {targets}"
    );
    assert_eq!(
        endpoints.len(),
        2,
        "each registration reports one endpoint: {targets}"
    );

    // The deregistration removes exactly what it names, and the describe stops reporting
    // it.
    expect_ok(
        "deregister the instance",
        call(
            &router,
            "DeregisterDBProxyTargets",
            &[
                ("DBProxyName", "emu-proxy"),
                ("DBInstanceIdentifiers.member.1", "emu-proxy-instance"),
            ],
        )
        .await,
    );
    let left = expect_ok(
        "the targets after the deregistration",
        call(
            &router,
            "DescribeDBProxyTargets",
            &[("DBProxyName", "emu-proxy")],
        )
        .await,
    );
    assert_eq!(
        texts(&left, "TrackedClusterId"),
        vec!["emu-proxy-cluster".to_string()],
        "the deregistered instance is gone and the cluster is left: {left}"
    );
    assert_eq!(
        texts(&left, "Type"),
        vec!["TRACKED_CLUSTER".to_string()],
        "the deregistered instance is gone and the cluster is left: {left}"
    );

    // The registration the proxy cannot take is the modelled already-registered fault: a
    // DB instance the proxy ALREADY REACHES, because the DB cluster that instance belongs
    // to is registered, cannot be registered on its own — the proxy is already associated
    // with that database, by a different registration than the one asked for.
    create_available_instance(
        &router,
        "emu-proxy-member",
        "aurora-mysql",
        &[("DBClusterIdentifier", "emu-proxy-cluster")],
    )
    .await;
    let refused = expect_error(
        "a DB instance the tracked cluster already reaches",
        call(
            &router,
            "RegisterDBProxyTargets",
            &[
                ("DBProxyName", "emu-proxy"),
                ("DBInstanceIdentifiers.member.1", "emu-proxy-member"),
            ],
        )
        .await,
        400,
        "DBProxyTargetAlreadyRegisteredFault",
    );
    assert_eq!(
        text(&refused, "Message"),
        DB_PROXY_TARGET_ALREADY_REGISTERED_MESSAGE,
        "expected the fault's own statement about the association, got {refused}"
    );
}

#[tokio::test]
async fn the_proxy_target_operations_keep_their_not_found_faults_apart() {
    let router = emulator();
    create_available_instance(&router, "emu-proxy-registered", "mysql", &[]).await;
    create_available_instance(&router, "emu-proxy-unregistered", "mysql", &[]).await;

    // Every proxy operation answers an unknown proxy name with the proxy's own not-found
    // fault.
    for (action, extra) in [
        ("RegisterDBProxyTargets", &[("DBInstanceIdentifiers.member.1", "emu-proxy-registered")][..]),
        ("DeregisterDBProxyTargets", &[("DBInstanceIdentifiers.member.1", "emu-proxy-registered")][..]),
        ("DescribeDBProxyTargets", &[][..]),
    ] {
        let mut params = vec![("DBProxyName", "emu-proxy-nope")];
        params.extend_from_slice(extra);
        expect_error(
            &format!("{action}: an unknown proxy"),
            call(&router, action, &params).await,
            404,
            "DBProxyNotFoundFault",
        );
    }
    expect_error(
        "DescribeDBProxies: an unknown proxy",
        call(
            &router,
            "DescribeDBProxies",
            &[("DBProxyName", "emu-proxy-nope")],
        )
        .await,
        404,
        "DBProxyNotFoundFault",
    );

    expect_ok(
        "create the proxy",
        call(
            &router,
            "CreateDBProxy",
            &[
                ("DBProxyName", "emu-proxy-faults"),
                ("EngineFamily", "POSTGRESQL"),
                ("RoleArn", PROXY_ROLE_ARN),
                ("VpcSubnetIds.member.1", SUBNET_ID),
            ],
        )
        .await,
    );
    expect_error(
        "a proxy name already taken",
        call(
            &router,
            "CreateDBProxy",
            &[
                ("DBProxyName", "emu-proxy-faults"),
                ("EngineFamily", "POSTGRESQL"),
                ("RoleArn", PROXY_ROLE_ARN),
                ("VpcSubnetIds.member.1", SUBNET_ID),
            ],
        )
        .await,
        400,
        "DBProxyAlreadyExistsFault",
    );
    expect_error(
        "a value that is not a subnet id",
        call(
            &router,
            "CreateDBProxy",
            &[
                ("DBProxyName", "emu-proxy-subnet"),
                ("EngineFamily", "MYSQL"),
                ("RoleArn", PROXY_ROLE_ARN),
                ("VpcSubnetIds.member.1", "not-a-subnet"),
            ],
        )
        .await,
        400,
        "InvalidSubnet",
    );
    wait_for_settle();

    // The target group a proxy HAS is the only one it has: any other name names none of
    // its target groups.
    for action in [
        "RegisterDBProxyTargets",
        "DeregisterDBProxyTargets",
        "DescribeDBProxyTargets",
    ] {
        expect_error(
            &format!("{action}: a target group that is not the proxy's"),
            call(
                &router,
                action,
                &[
                    ("DBProxyName", "emu-proxy-faults"),
                    ("TargetGroupName", "emu-other-group"),
                    ("DBInstanceIdentifiers.member.1", "emu-proxy-registered"),
                ],
            )
            .await,
            404,
            "DBProxyTargetGroupNotFoundFault",
        );
    }
    // The documented name is accepted.
    expect_ok(
        "the proxy's own target group",
        call(
            &router,
            "RegisterDBProxyTargets",
            &[
                ("DBProxyName", "emu-proxy-faults"),
                ("TargetGroupName", DEFAULT_DB_PROXY_TARGET_GROUP),
                ("DBInstanceIdentifiers.member.1", "emu-proxy-registered"),
            ],
        )
        .await,
    );
    // A database this service holds no record of cannot be registered, and one that is
    // not registered cannot be deregistered — two different misses, two different faults.
    for (case, member, identifier, code) in [
        (
            "an instance that does not exist",
            "DBInstanceIdentifiers.member.1",
            "emu-proxy-missing",
            "DBInstanceNotFound",
        ),
        (
            "a cluster that does not exist",
            "DBClusterIdentifiers.member.1",
            "emu-proxy-missing",
            "DBClusterNotFoundFault",
        ),
    ] {
        expect_error(
            case,
            call(
                &router,
                "RegisterDBProxyTargets",
                &[("DBProxyName", "emu-proxy-faults"), (member, identifier)],
            )
            .await,
            404,
            code,
        );
    }
    expect_error(
        "a database that is not registered",
        call(
            &router,
            "DeregisterDBProxyTargets",
            &[
                ("DBProxyName", "emu-proxy-faults"),
                ("DBInstanceIdentifiers.member.1", "emu-proxy-unregistered"),
            ],
        )
        .await,
        404,
        "DBProxyTargetNotFoundFault",
    );
    // The registration that failed changed nothing: the one target registered above is
    // still the only one.
    let targets = expect_ok(
        "the targets after the refused calls",
        call(
            &router,
            "DescribeDBProxyTargets",
            &[("DBProxyName", "emu-proxy-faults")],
        )
        .await,
    );
    assert_eq!(
        texts(&targets, "RdsResourceId").len(),
        1,
        "a refused registration leaves the target group as it was: {targets}"
    );
}

// ---------------------------------------------------------------------------
// DescribeDBClusterAutomatedBackups
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cluster_automated_backups_are_projected_from_the_cluster_they_are_of() {
    let router = emulator();
    create_available_cluster(
        &router,
        "emu-backup-cluster",
        &[("BackupRetentionPeriod", "7"), ("StorageEncrypted", "true")],
    )
    .await;
    let cluster = describe_cluster(&router, "emu-backup-cluster").await;
    let resource_id = text(&cluster, "DbClusterResourceId");

    // Each form the report is narrowed by names the one cluster's backup, and every
    // member is the cluster's own.
    for (case, params) in [
        ("the whole region", vec![]),
        (
            "by DB cluster identifier",
            vec![("DBClusterIdentifier", "emu-backup-cluster")],
        ),
        (
            "by the cluster's ARN",
            vec![(
                "DBClusterIdentifier",
                // The member accepts the ARN form too.
                "arn:aws:rds:us-west-2:000000000000:cluster:emu-backup-cluster",
            )],
        ),
        (
            "by DB cluster resource id",
            vec![("DbClusterResourceId", resource_id.as_str())],
        ),
        (
            "by the db-cluster-id filter",
            vec![
                ("Filters.Filter.1.Name", "db-cluster-id"),
                ("Filters.Filter.1.Values.Value.1", "emu-backup-cluster"),
            ],
        ),
        (
            "by the db-cluster-resource-id filter",
            vec![
                ("Filters.Filter.1.Name", "db-cluster-resource-id"),
                ("Filters.Filter.1.Values.Value.1", resource_id.as_str()),
            ],
        ),
    ] {
        let reported = expect_ok(
            case,
            call(&router, "DescribeDBClusterAutomatedBackups", &params).await,
        );
        assert_eq!(
            text(&reported, "DBClusterIdentifier"),
            "emu-backup-cluster",
            "case {case:?}: the backup names the cluster it is of: {reported}"
        );
        assert_eq!(
            text(&reported, "Status"),
            DB_CLUSTER_AUTOMATED_BACKUP_STATUS_ACTIVE,
            "case {case:?}: the backup of a cluster that is still here is active: {reported}"
        );
        assert_eq!(
            text(&reported, "BackupRetentionPeriod"),
            "7",
            "case {case:?}: the retention period is the cluster's own: {reported}"
        );
        assert_eq!(
            text(&reported, "DbClusterResourceId"),
            resource_id,
            "case {case:?}: {reported}"
        );
        assert_eq!(
            text(&reported, "Engine"),
            text(&cluster, "Engine"),
            "case {case:?}: {reported}"
        );
        assert_eq!(
            text(&reported, "StorageEncrypted"),
            "true",
            "case {case:?}: {reported}"
        );
        assert_eq!(
            text(&reported, "DBClusterAutomatedBackupsArn"),
            format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster-auto-backup:{resource_id}"),
            "case {case:?}: {reported}"
        );
        assert_eq!(
            texts(&reported, "EarliestTime").len(),
            1,
            "case {case:?}: the backup reports the window it can be restored to: {reported}"
        );
    }

    // A value that resolves to no automated backup is the modelled not-found fault, in
    // each of the two members that narrow the report.
    for (case, member, value) in [
        ("an unknown cluster identifier", "DBClusterIdentifier", "emu-backup-nope"),
        (
            "an unknown cluster resource id",
            "DbClusterResourceId",
            "cluster-NOTHINGHEREATALL00000000",
        ),
    ] {
        expect_error(
            case,
            call(
                &router,
                "DescribeDBClusterAutomatedBackups",
                &[(member, value)],
            )
            .await,
            404,
            "DBClusterAutomatedBackupNotFoundFault",
        );
    }
    expect_error(
        "an unsupported filter name",
        call(
            &router,
            "DescribeDBClusterAutomatedBackups",
            &[
                ("Filters.Filter.1.Name", "cluster"),
                ("Filters.Filter.1.Values.Value.1", "emu-backup-cluster"),
            ],
        )
        .await,
        400,
        "InvalidParameterValue",
    );
    // The one status the filter documents is `retained`, the status of a backup whose
    // cluster has been deleted: no backup reported here has it.
    let retained = expect_ok(
        "the status filter",
        call(
            &router,
            "DescribeDBClusterAutomatedBackups",
            &[
                ("Filters.Filter.1.Name", "status"),
                ("Filters.Filter.1.Values.Value.1", "retained"),
            ],
        )
        .await,
    );
    assert!(
        texts(&retained, "DBClusterIdentifier").is_empty(),
        "the backup of a cluster that is still here is not retained: {retained}"
    );
}

// ---------------------------------------------------------------------------
// DescribeDBLogFiles / DownloadDBLogFilePortion
// ---------------------------------------------------------------------------

/// The engines the log-file name table is driven over, with the file name each publishes.
/// The names come from `crate::state::engine_log::log_file_name`, whose own doc cites the
/// per-engine log files Amazon RDS publishes; this row set covers an engine family the
/// issue did not cite (SQL Server and Oracle) beside the two it did.
const LOG_FILE_ENGINES: &[(&str, &str)] = &[
    ("mysql", "error/mysql-error.log"),
    ("postgres", "error/postgresql.log"),
    ("mariadb", "error/mysql-error.log"),
    ("sqlserver-ex", "log/ERRORLOG"),
    ("oracle-ee", "trace/alert.log"),
];

#[tokio::test]
async fn log_file_access_reports_the_file_each_engine_writes() {
    let router = emulator();

    for (index, (engine, expected_name)) in LOG_FILE_ENGINES.iter().enumerate() {
        let identifier = format!("emu-log-{index}");
        let case = format!("{engine} log file");
        assert_eq!(
            engine_log::log_file_name(engine),
            *expected_name,
            "case {case:?}: the table and the service must name the same file"
        );
        create_available_instance(&router, &identifier, engine, &[]).await;

        let reported = expect_ok(
            &case,
            call(
                &router,
                "DescribeDBLogFiles",
                &[("DBInstanceIdentifier", identifier.as_str())],
            )
            .await,
        );
        assert_eq!(
            text(&reported, "LogFileName"),
            *expected_name,
            "case {case:?}: {reported}"
        );
        let size: i64 = text(&reported, "Size")
            .parse()
            .expect("the reported size is a number");
        assert!(
            size > 0,
            "case {case:?}: the file carries the lines this service recorded: {reported}"
        );
        let last_written: i64 = text(&reported, "LastWritten")
            .parse()
            .expect("the reported timestamp is a number");
        assert!(
            last_written > 0,
            "case {case:?}: the file reports when its last line was written: {reported}"
        );

        // The download serves those lines, and its size is the size the list reported.
        let downloaded = expect_ok(
            &case,
            call(
                &router,
                "DownloadDBLogFilePortion",
                &[
                    ("DBInstanceIdentifier", identifier.as_str()),
                    ("LogFileName", expected_name),
                ],
            )
            .await,
        );
        let data = text(&downloaded, "LogFileData");
        assert_eq!(
            i64::try_from(data.len()).expect("a portion shorter than i64::MAX"),
            size,
            "case {case:?}: the download serves the file the list measured: {downloaded}"
        );
        assert!(
            data.contains(&identifier) && data.contains("available"),
            "case {case:?}: the file records this instance's own lifecycle: {data}"
        );
        assert_eq!(
            text(&downloaded, "AdditionalDataPending"),
            "false",
            "case {case:?}: the whole file was served: {downloaded}"
        );
    }
}

#[tokio::test]
async fn log_file_access_narrows_pages_and_refuses_what_it_cannot_serve() {
    let router = emulator();
    create_available_instance(&router, "emu-log-paging", "mysql", &[]).await;
    let log_file_name = engine_log::log_file_name("mysql");

    // `FilenameContains`, `FileSize` and `FileLastWritten` each narrow the list, and a
    // value the file does not satisfy drops it.
    for (case, params, expect_listed) in [
        (
            "FilenameContains matching the file",
            vec![("FilenameContains", "mysql")],
            true,
        ),
        (
            "FilenameContains matching nothing",
            vec![("FilenameContains", "postgres")],
            false,
        ),
        ("FileSize below the file", vec![("FileSize", "0")], true),
        (
            "FileSize above the file",
            vec![("FileSize", "1000000")],
            false,
        ),
        (
            "FileLastWritten before the file",
            vec![("FileLastWritten", "1")],
            true,
        ),
        (
            "FileLastWritten after the file",
            vec![("FileLastWritten", "99999999999999")],
            false,
        ),
    ] {
        let mut request = vec![("DBInstanceIdentifier", "emu-log-paging")];
        request.extend(params.iter().copied());
        let reported = expect_ok(case, call(&router, "DescribeDBLogFiles", &request).await);
        assert_eq!(
            texts(&reported, "LogFileName").is_empty(),
            !expect_listed,
            "case {case:?}: {reported}"
        );
    }
    expect_error(
        "a filter, which this operation does not support",
        call(
            &router,
            "DescribeDBLogFiles",
            &[
                ("DBInstanceIdentifier", "emu-log-paging"),
                ("Filters.Filter.1.Name", "log-file"),
                ("Filters.Filter.1.Values.Value.1", "error"),
            ],
        )
        .await,
        400,
        "InvalidParameterValue",
    );

    // The download pages by LINE: the first portion carries `NumberOfLines` of them and
    // hands back the token the next one resumes from.
    let first = expect_ok(
        "the first line",
        call(
            &router,
            "DownloadDBLogFilePortion",
            &[
                ("DBInstanceIdentifier", "emu-log-paging"),
                ("LogFileName", log_file_name),
                ("NumberOfLines", "1"),
            ],
        )
        .await,
    );
    assert_eq!(
        text(&first, "LogFileData").lines().count(),
        1,
        "NumberOfLines bounds the portion: {first}"
    );
    assert_eq!(
        text(&first, "AdditionalDataPending"),
        "true",
        "there are lines left to read: {first}"
    );
    let marker = text(&first, "Marker");
    let rest = expect_ok(
        "the rest of the file",
        call(
            &router,
            "DownloadDBLogFilePortion",
            &[
                ("DBInstanceIdentifier", "emu-log-paging"),
                ("LogFileName", log_file_name),
                ("Marker", marker.as_str()),
            ],
        )
        .await,
    );
    assert_eq!(
        text(&rest, "AdditionalDataPending"),
        "false",
        "the portion after the token runs to the end: {rest}"
    );
    assert!(
        !text(&rest, "LogFileData").is_empty()
            && text(&rest, "LogFileData") != text(&first, "LogFileData"),
        "the second portion is the lines the first did not serve: {rest}"
    );
    // A token this service never handed out, and a file this instance does not write, are
    // each refused rather than answered with a portion of something else.
    expect_error(
        "a marker no prior request issued",
        call(
            &router,
            "DownloadDBLogFilePortion",
            &[
                ("DBInstanceIdentifier", "emu-log-paging"),
                ("LogFileName", log_file_name),
                ("Marker", "not-a-marker"),
            ],
        )
        .await,
        400,
        "InvalidParameterValue",
    );
    expect_error(
        "a log file this instance does not write",
        call(
            &router,
            "DownloadDBLogFilePortion",
            &[
                ("DBInstanceIdentifier", "emu-log-paging"),
                ("LogFileName", "error/postgresql.log"),
            ],
        )
        .await,
        404,
        "DBLogFileNotFoundFault",
    );
}

#[tokio::test]
async fn log_file_access_does_not_reach_an_instance_that_is_still_being_created() {
    let router = emulator();
    // Deliberately NOT waited out: the instance is still `creating`, the status the clock
    // has yet to settle.
    expect_ok(
        "create the instance",
        call(
            &router,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "emu-log-creating"),
                ("Engine", "mysql"),
                ("DBInstanceClass", "db.m5.large"),
            ],
        )
        .await,
    );
    let log_file_name = engine_log::log_file_name("mysql");

    // Both log operations answer the instance that is not yet there, and the identifier
    // that never existed, with the same modelled fault — and word the two misses the way
    // the service words them.
    for (case, action, extra) in [
        ("DescribeDBLogFiles", "DescribeDBLogFiles", &[][..]),
        (
            "DownloadDBLogFilePortion",
            "DownloadDBLogFilePortion",
            &[("LogFileName", log_file_name)][..],
        ),
    ] {
        let mut not_ready = vec![("DBInstanceIdentifier", "emu-log-creating")];
        not_ready.extend_from_slice(extra);
        let refused = expect_error(
            &format!("{case}: an instance still being created"),
            call(&router, action, &not_ready).await,
            404,
            "DBInstanceNotFound",
        );
        assert_eq!(
            text(&refused, "Message"),
            "DBInstance emu-log-creating not found.",
            "case {case:?}: {refused}"
        );

        let mut never_existed = vec![("DBInstanceIdentifier", "emu-log-nope")];
        never_existed.extend_from_slice(extra);
        let refused = expect_error(
            &format!("{case}: an identifier that never existed"),
            call(&router, action, &never_existed).await,
            404,
            "DBInstanceNotFound",
        );
        assert_eq!(
            text(&refused, "Message"),
            "DBInstance not found: emu-log-nope",
            "case {case:?}: {refused}"
        );
    }

    // Once the instance has come up, the same call is served.
    wait_for_settle();
    let reported = expect_ok(
        "the instance once it is available",
        call(
            &router,
            "DescribeDBLogFiles",
            &[("DBInstanceIdentifier", "emu-log-creating")],
        )
        .await,
    );
    assert_eq!(
        text(&reported, "LogFileName"),
        log_file_name,
        "an available instance's log file is listed: {reported}"
    );
}
