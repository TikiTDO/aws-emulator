// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the words this front door refuses a request IN, and for the inputs
//! it refuses at all.
//!
//! Every test here pins one rule over a TABLE of inputs rather than one call, because the
//! rules are about classes of request:
//!
//! 1. A fault one condition can be answered by is worded by the ROLE the resource plays in
//!    the call: a call that CHANGES a DB security group says `DBSecurityGroup not found:
//!    <name>`, while a call that REPORTS one says `DBSecurityGroup <name> not found.`
//! 2. A `DBProxyName` that resolves no proxy is refused with the requested name in single
//!    quotes, on every operation that resolves one.
//! 3. A member that NARROWS a report is a predicate: a value it matches nothing with is an
//!    empty success, not a fault — while a value outside a bound the MODEL declares is
//!    refused at the boundary, in the constraint-violation wording AWS answers with.
//! 4. An `Engine` that RESOLVES an engine family is refused when it names no engine this
//!    front door publishes.
//! 5. A `SourceType` outside the published event source types is refused with the one
//!    sentence that names the valid types and does not echo the value.
//! 6. A parameter value outside its published domain is refused in the wording that
//!    domain's FORM takes: a numeric range and an enumerated value list get different
//!    sentences, both naming the value, the parameter and its `AllowedValues`.
//! 7. A reboot is accepted only from the statuses RDS lists in its own refusal, which names
//!    that list, the instance and the status it is actually in.
//! 8. A required description that is present but EMPTY is refused rather than stored.
//!
//! Every call goes over the wire through the shared harness in `crate::db_instances`, so
//! each assertion reads the XML this emulator itself answered with.

use service_com_amazonaws_rds_amazonrdsv19::state::{
    check_not_empty, db_proxy_not_found_message, db_security_group_not_found_colon_message,
    db_security_group_not_found_message, invalid_event_source_type_message,
    reboot_db_instance_state_message, DB_INSTANCE_ENGINES, EVENT_CATEGORIES,
    INVALID_DB_ENGINE_MESSAGE, REBOOTABLE_DB_INSTANCE_STATUSES, SIBLING_CONTROL_PLANE_ENGINES,
};

use crate::db_instances::{
    call, emulator, expect_error, expect_ok, text, texts, wait_for_settle,
};

/// The Query API code this front door answers a value its member does not accept with.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// An IAM role ARN and an EC2 subnet id a proxy create needs; neither is resolved by this
/// service, which holds no roles and no subnets.
const PROXY_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/rds-proxy";
const SUBNET_ID: &str = "subnet-0a1b2c3d";

// ---------------------------------------------------------------------------
// 1. The DB security group not-found wordings: mutate path versus read path
// ---------------------------------------------------------------------------

/// The names the tables below address a group that was never created by: a plain one, one
/// sent in mixed case (the member "is stored as a lowercase string", so the refusal names
/// the folded spelling) and the shortest one a request can carry.
const ABSENT_GROUP_NAMES: [&str; 3] = ["emu-wording-absent", "Emu-Wording-Absent-Too", "x"];

/// Every operation that CHANGES the DB security group it names answers a name that resolves
/// no group with the mutate-path wording — and the report of the same condition answers the
/// read-path wording, which is the contrast that makes the rule a rule.
#[tokio::test]
async fn the_not_found_wording_follows_the_role_the_group_plays_in_the_call() {
    let router = emulator();

    // (the action, the members it carries beyond the group name)
    let mutating: [(&str, &[(&str, &str)]); 3] = [
        ("AuthorizeDBSecurityGroupIngress", &[("CIDRIP", "203.0.113.0/24")]),
        ("RevokeDBSecurityGroupIngress", &[("CIDRIP", "203.0.113.0/24")]),
        ("DeleteDBSecurityGroup", &[]),
    ];
    for (action, extra) in mutating {
        for named in ABSENT_GROUP_NAMES {
            let case = format!("{action} on the absent {named:?}");
            let mut params = vec![("DBSecurityGroupName", named.to_string())];
            params.extend(
                extra
                    .iter()
                    .map(|(member, value)| (*member, (*value).to_string())),
            );
            let body = expect_error(
                &case,
                call(&router, action, &params).await,
                404,
                "DBSecurityGroupNotFound",
            );
            let expected = db_security_group_not_found_colon_message(&named.to_ascii_lowercase());
            let message = text(&body, "Message");
            assert_eq!(
                message, expected,
                "case {case:?}: expected the mutate-path wording {expected:?}, got {message:?}"
            );
        }
    }

    for named in ABSENT_GROUP_NAMES {
        let case = format!("DescribeDBSecurityGroups for the absent {named:?}");
        let body = expect_error(
            &case,
            call(
                &router,
                "DescribeDBSecurityGroups",
                &[("DBSecurityGroupName", named)],
            )
            .await,
            404,
            "DBSecurityGroupNotFound",
        );
        let expected = db_security_group_not_found_message(&named.to_ascii_lowercase());
        let message = text(&body, "Message");
        assert_eq!(
            message, expected,
            "case {case:?}: expected the read-path wording {expected:?}, got {message:?}"
        );
    }

    // The two wordings are different sentences about the same condition, which is the whole
    // point of keeping both.
    assert_ne!(
        db_security_group_not_found_colon_message("emu-wording-absent"),
        db_security_group_not_found_message("emu-wording-absent"),
        "the mutate-path and read-path wordings must not be the same sentence"
    );
}

// ---------------------------------------------------------------------------
// 2. The DB proxy not-found wording
// ---------------------------------------------------------------------------

/// Every operation that resolves a `DBProxyName` quotes the requested name in the refusal,
/// and a proxy that EXISTS is reported by the same operation — so the refusal is about
/// resolution and not about the member.
#[tokio::test]
async fn a_db_proxy_name_that_resolves_no_proxy_is_quoted_in_the_refusal() {
    let router = emulator();
    expect_ok(
        "create the proxy the positive control reads",
        call(
            &router,
            "CreateDBProxy",
            &[
                ("DBProxyName", "emu-wording-proxy"),
                ("EngineFamily", "POSTGRESQL"),
                ("RoleArn", PROXY_ROLE_ARN),
                ("VpcSubnetIds.member.1", SUBNET_ID),
            ],
        )
        .await,
    );

    // (the action, the members it carries beyond the proxy name)
    let actions: [(&str, &[(&str, &str)]); 5] = [
        ("DescribeDBProxies", &[]),
        ("DescribeDBProxyTargets", &[]),
        ("DescribeDBProxyTargetGroups", &[]),
        (
            "RegisterDBProxyTargets",
            &[("DBInstanceIdentifiers.member.1", "emu-wording-instance")],
        ),
        (
            "DeregisterDBProxyTargets",
            &[("DBInstanceIdentifiers.member.1", "emu-wording-instance")],
        ),
    ];
    // `DBProxyName` is not folded, so each spelling is a different proxy name — and none of
    // them was created.
    for named in ["emu-wording-noproxy", "Emu-Wording-Proxy", "p"] {
        for (action, extra) in actions {
            let case = format!("{action} on the absent proxy {named:?}");
            let mut params = vec![("DBProxyName", named.to_string())];
            params.extend(
                extra
                    .iter()
                    .map(|(member, value)| (*member, (*value).to_string())),
            );
            let body = expect_error(
                &case,
                call(&router, action, &params).await,
                404,
                "DBProxyNotFoundFault",
            );
            let expected = db_proxy_not_found_message(named);
            let message = text(&body, "Message");
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
            assert!(
                expected.contains(&format!("'{named}'")),
                "case {case:?}: the wording must quote the requested name, got {expected:?}"
            );
        }
    }

    let reported = expect_ok(
        "DescribeDBProxies for the proxy that exists",
        call(
            &router,
            "DescribeDBProxies",
            &[("DBProxyName", "emu-wording-proxy")],
        )
        .await,
    );
    assert_eq!(
        text(&reported, "DBProxyName"),
        "emu-wording-proxy",
        "case \"the proxy that exists\": expected the created proxy to be reported, got \
         {reported}"
    );
}

// ---------------------------------------------------------------------------
// 3. DescribeExportTasks: a narrowing member versus a modelled bound
// ---------------------------------------------------------------------------

/// `ExportTaskIdentifier` narrows the report, so an identifier that names no export task is
/// an empty 200; `MaxRecords` carries a `@range` the model declares, so a value outside it
/// is refused in the constraint-violation wording, and each published bound is served.
#[tokio::test]
async fn describe_export_tasks_filters_by_identifier_and_bounds_its_page_size() {
    let router = emulator();

    for named in ["emu-no-such-export", "Emu-No-Such-Export", "e"] {
        let case = format!("the identifier {named:?}");
        let body = expect_ok(
            &case,
            call(
                &router,
                "DescribeExportTasks",
                &[("ExportTaskIdentifier", named)],
            )
            .await,
        );
        assert!(
            texts(&body, "ExportTask").is_empty(),
            "case {case:?}: an identifier that names no export task must report an empty \
             list, got {body}"
        );
    }

    // `DescribeExportTasks$MaxRecords` is bounded by the model (range 20..100), so a value
    // outside it is refused before the handler runs — with AWS's constraint-violation
    // clause, naming the value, the member in its wire spelling and the bound it broke.
    // (case, the value, the requirement the clause states)
    let refused: [(&str, i32, String); 4] = [
        (
            "well below the minimum",
            5,
            "Member must have value greater than or equal to 20".to_string(),
        ),
        (
            "one below the minimum",
            19,
            "Member must have value greater than or equal to 20".to_string(),
        ),
        (
            "one above the maximum",
            101,
            "Member must have value less than or equal to 100".to_string(),
        ),
        (
            "a negative page size",
            -1,
            "Member must have value greater than or equal to 20".to_string(),
        ),
    ];
    for (case, value, requirement) in refused {
        let body = expect_error(
            case,
            call(
                &router,
                "DescribeExportTasks",
                &[("MaxRecords", value.to_string())],
            )
            .await,
            400,
            INVALID_PARAMETER_VALUE,
        );
        let expected = format!(
            "1 validation error detected: Value '{value}' at 'maxRecords' failed to satisfy \
             constraint: {requirement}"
        );
        let message = text(&body, "Message");
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }

    for bound in ["20", "100"] {
        let case = format!("MaxRecords at the published bound {bound}");
        expect_ok(
            &case,
            call(&router, "DescribeExportTasks", &[("MaxRecords", bound)]).await,
        );
    }
}

// ---------------------------------------------------------------------------
// 4. An Engine that names no engine
// ---------------------------------------------------------------------------

/// The reports whose `Engine` RESOLVES the engine whose catalogue is reported refuse a value
/// that names no engine this front door publishes, and serve every one that does —
/// whichever case it is written in. Each one words that refusal the way the real service
/// words it for that operation: the engine-version report names the value it did not
/// recognise, the orderable-options report answers its one fixed sentence.
#[tokio::test]
async fn an_engine_that_names_no_published_engine_is_refused() {
    let router = emulator();

    for action in ["DescribeDBEngineVersions", "DescribeOrderableDBInstanceOptions"] {
        // Values spanning the ways a value can fail to be an engine: a name nothing
        // publishes, a published name with a version glued on, a published name with a
        // stray character, a published name of a DIFFERENT API's vocabulary
        // (`EngineFamily`), and the empty string.
        for value in ["not-an-engine", "mysql8.0", "mysql-", "MYSQL_COMPATIBLE", ""] {
            let case = format!("{action} with Engine {value:?}");
            let body = expect_error(
                &case,
                call(&router, action, &[("Engine", value)]).await,
                400,
                INVALID_PARAMETER_VALUE,
            );
            let expected = if action == "DescribeOrderableDBInstanceOptions" {
                INVALID_DB_ENGINE_MESSAGE.to_string()
            } else {
                format!("Unrecognized engine name: {value}")
            };
            let message = text(&body, "Message");
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        }

        // The positive control: every engine the member publishes is served, and the name
        // is resolved case-insensitively like every other per-engine rule here. The engine
        // list is this service's own published vocabulary
        // (`smithy_query(query="doc",shape="CreateDBInstanceMessage",member="Engine")` and
        // `…shape="CreateDBCapacityReservationRequest",member="Engine"`, plus the two
        // sibling control planes this front door serves).
        for engine in DB_INSTANCE_ENGINES
            .iter()
            .chain(SIBLING_CONTROL_PLANE_ENGINES.iter())
        {
            for spelling in [engine.to_string(), engine.to_ascii_uppercase()] {
                let case = format!("{action} with Engine {spelling:?}");
                expect_ok(&case, call(&router, action, &[("Engine", &spelling)]).await);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 5. The invalid event-source-type refusal
// ---------------------------------------------------------------------------

/// A `SourceType` outside the published set is refused with the one fixed sentence, which
/// names the valid types and says nothing about the value sent; every published type is
/// reported with its own category map.
#[tokio::test]
async fn an_event_source_type_outside_the_published_set_is_refused_with_the_fixed_sentence() {
    let router = emulator();

    // Values spanning the ways a source type can be one this report publishes no category
    // map for: a name nothing publishes, a name of the published set in the wrong case ("the
    // documented values are all lowercase"), a near miss of a published one, a name the
    // front door ACCEPTS but which has no published category map (the "Feature may be
    // disabled." case), and the empty string.
    for value in [
        "not-a-real-source-type",
        "DB-Instance",
        "db-instance-group",
        "zero-etl",
        "",
    ] {
        let case = format!("SourceType {value:?}");
        let body = expect_error(
            &case,
            call(&router, "DescribeEventCategories", &[("SourceType", value)]).await,
            400,
            INVALID_PARAMETER_VALUE,
        );
        let message = text(&body, "Message");
        assert_eq!(
            message,
            invalid_event_source_type_message(),
            "case {case:?}: expected the fixed sentence, got {message:?}"
        );
        assert!(
            message.starts_with(
                "Invalid event source type. Feature may be disabled. Valid types are "
            ),
            "case {case:?}: expected AWS's fixed prefix, got {message:?}"
        );
        // The sentence is fixed and names the source types the front door accepts, so it
        // carries the rejected value only where that value is one of the names it lists
        // anyway — every other row proves it echoes nothing of the request.
        assert!(
            value.is_empty()
                || ::service_common::rds_event_source_types::EVENT_SOURCE_TYPES
                    .contains(&value)
                || !message.contains(value),
            "case {case:?}: the sentence must not echo the offending value, got {message:?}"
        );
        // Each valid type is named, in single quotes, after that prefix.
        for (source_type, _) in EVENT_CATEGORIES {
            assert!(
                message.contains(&format!("'{source_type}'")),
                "case {case:?}: expected the sentence to name '{source_type}', got {message:?}"
            );
        }
    }

    for (source_type, categories) in EVENT_CATEGORIES {
        let case = format!("the published source type {source_type:?}");
        let body = expect_ok(
            &case,
            call(
                &router,
                "DescribeEventCategories",
                &[("SourceType", *source_type)],
            )
            .await,
        );
        assert_eq!(
            texts(&body, "SourceType"),
            vec![(*source_type).to_string()],
            "case {case:?}: expected exactly that source type's map, got {body}"
        );
        assert_eq!(
            texts(&body, "EventCategory").len(),
            categories.len(),
            "case {case:?}: expected its {} published categories, got {body}",
            categories.len()
        );
    }
}

// ---------------------------------------------------------------------------
// 6. The two wordings of a value outside a parameter's published domain
// ---------------------------------------------------------------------------

/// One row of the domain table: the operation that writes the value, the group's family, the
/// parameter, its published `AllowedValues` and a value outside them.
struct DomainRow {
    case: &'static str,
    /// `ModifyDBParameterGroup` or `ModifyDBClusterParameterGroup` — the two operations that
    /// write a parameter value, over the two catalogues.
    cluster_level: bool,
    family: &'static str,
    parameter: &'static str,
    domain: &'static str,
    refused: &'static str,
}

/// The rows span the two FORMS a published domain takes (a comma-separated value list and a
/// `<low>-<high>` numeric range), each at both parameter levels, with the integer, float and
/// string data types represented — the axes the wording rule generalises over. The domains
/// are the ones `DescribeEngineDefaultParameters` /
/// `DescribeEngineDefaultClusterParameters` report for the family under `AllowedValues`,
/// which the test re-reads from the service for every row.
const DOMAIN_ROWS: &[DomainRow] = &[
    DomainRow {
        case: "a PostgreSQL value list",
        cluster_level: false,
        family: "postgres16",
        parameter: "log_statement",
        domain: "none,ddl,mod,all",
        refused: "not_a_valid_enum_value",
    },
    DomainRow {
        case: "a MySQL value list of integers",
        cluster_level: false,
        family: "mysql8.0",
        parameter: "innodb_flush_log_at_trx_commit",
        domain: "0,1,2",
        refused: "3",
    },
    DomainRow {
        case: "a MySQL integer range",
        cluster_level: false,
        family: "mysql8.0",
        parameter: "innodb_lock_wait_timeout",
        domain: "1-1073741824",
        refused: "0",
    },
    DomainRow {
        case: "a PostgreSQL range whose low bound is negative",
        cluster_level: false,
        family: "postgres16",
        parameter: "log_min_duration_statement",
        domain: "-1-2147483647",
        refused: "-2",
    },
    DomainRow {
        case: "a cluster-level value list",
        cluster_level: true,
        family: "docdb5.0",
        parameter: "tls",
        domain: "disabled,enabled,fips-140-3,tls1.2+,tls1.3+",
        refused: "maybe",
    },
    DomainRow {
        case: "a cluster-level integer range",
        cluster_level: true,
        family: "docdb5.0",
        parameter: "change_stream_log_retention_duration",
        domain: "3600-86400",
        refused: "100",
    },
    DomainRow {
        case: "a cluster-level float range",
        cluster_level: true,
        family: "docdb5.0",
        parameter: "profiler_sampling_rate",
        domain: "0.0-1.0",
        refused: "1.5",
    },
];

/// A value outside a parameter's published domain is refused in the wording that domain's
/// FORM takes, on both parameter-group levels, and the refused write stores nothing.
#[tokio::test]
async fn a_value_outside_a_published_domain_is_refused_in_that_domains_own_wording() {
    let router = emulator();

    for (row_index, row) in DOMAIN_ROWS.iter().enumerate() {
        let group = format!("emu-domain-{row_index}");
        let (create, modify, describe, name_member) = if row.cluster_level {
            (
                "CreateDBClusterParameterGroup",
                "ModifyDBClusterParameterGroup",
                "DescribeDBClusterParameters",
                "DBClusterParameterGroupName",
            )
        } else {
            (
                "CreateDBParameterGroup",
                "ModifyDBParameterGroup",
                "DescribeDBParameters",
                "DBParameterGroupName",
            )
        };
        expect_ok(
            row.case,
            call(
                &router,
                create,
                &[
                    (name_member, group.as_str()),
                    ("DBParameterGroupFamily", row.family),
                    ("Description", "a domain probe"),
                ],
            )
            .await,
        );

        // The domain the refusal must name is the one this service itself reports for the
        // parameter, read back rather than assumed.
        let reported = expect_ok(
            row.case,
            call(
                &router,
                describe,
                &[(name_member, group.as_str()), ("Source", "engine-default")],
            )
            .await,
        );
        assert!(
            reported.contains(&format!("<AllowedValues>{}</AllowedValues>", row.domain)),
            "case {:?}: the family must publish {:?} for {}, got {reported}",
            row.case,
            row.domain,
            row.parameter
        );

        let body = expect_error(
            row.case,
            call(
                &router,
                modify,
                &[
                    (name_member, group.as_str()),
                    ("Parameters.Parameter.1.ParameterName", row.parameter),
                    ("Parameters.Parameter.1.ParameterValue", row.refused),
                    ("Parameters.Parameter.1.ApplyMethod", "immediate"),
                ],
            )
            .await,
            400,
            INVALID_PARAMETER_VALUE,
        );
        let expected = if row.domain.contains(',') {
            format!(
                "Invalid parameter value: {} for: {} allowed values are: {}",
                row.refused, row.parameter, row.domain
            )
        } else {
            format!(
                "Value: {} is outside of range: {} for parameter: {}",
                row.refused, row.domain, row.parameter
            )
        };
        let message = text(&body, "Message");
        assert_eq!(
            message, expected,
            "case {:?}: expected {expected:?}, got {message:?}",
            row.case
        );

        // A refused write leaves the group carrying no value of the caller's for it.
        let after = expect_ok(
            row.case,
            call(
                &router,
                describe,
                &[(name_member, group.as_str()), ("Source", "user")],
            )
            .await,
        );
        assert!(
            !after.contains(&format!("<ParameterValue>{}</ParameterValue>", row.refused)),
            "case {:?}: a refused write must store nothing, got {after}",
            row.case
        );
    }
}

// ---------------------------------------------------------------------------
// 7. The reboot state gate
// ---------------------------------------------------------------------------

/// A reboot asked of an instance in a status it is not accepted from is refused with the
/// message that lists the permitted statuses, names the instance and names the status it is
/// in; from `available` it is accepted.
#[tokio::test]
async fn a_reboot_outside_the_accepted_statuses_names_the_statuses_it_is_accepted_from() {
    let router = emulator();

    // Each row leaves its instance in a different status this reboot cannot be served
    // from: one still being created, one that has been stopped, and one already rebooting.
    // (the identifier, what to do before the reboot, the status the instance then reports)
    let rows: [(&str, &[&str], &str); 3] = [
        ("emu-reboot-creating", &[], "creating"),
        ("emu-reboot-stopped", &["StopDBInstance"], "stopped"),
        ("emu-reboot-rebooting", &["RebootDBInstance"], "rebooting"),
    ];
    for (identifier, before, status) in rows {
        let case = format!("a reboot of {identifier} while {status}");
        expect_ok(
            &case,
            call(
                &router,
                "CreateDBInstance",
                &[
                    ("DBInstanceIdentifier", identifier),
                    ("Engine", "postgres"),
                    ("DBInstanceClass", "db.t3.micro"),
                ],
            )
            .await,
        );
        if !before.is_empty() {
            // The preparation runs against the instance once it is up.
            wait_for_settle();
            for action in before {
                expect_ok(
                    &case,
                    call(&router, action, &[("DBInstanceIdentifier", identifier)]).await,
                );
            }
            if before == ["StopDBInstance"] {
                // A stop settles on `stopped`; a reboot's own `rebooting` is still in force
                // when the next call arrives, which is the third row.
                wait_for_settle();
            }
        }

        let body = expect_error(
            &case,
            call(
                &router,
                "RebootDBInstance",
                &[("DBInstanceIdentifier", identifier)],
            )
            .await,
            400,
            "InvalidDBInstanceState",
        );
        let message = text(&body, "Message");
        let expected = format!(
            "Can only reboot db instances with state in: available, storage-optimization, \
             storage-initialization, incompatible-credentials, incompatible-parameters.  \
             Instance {identifier} has state: {status}."
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        // The same sentence the service builds from its own permitted-status list, so the
        // list and the wording cannot drift apart.
        assert_eq!(
            message,
            reboot_db_instance_state_message(identifier, status),
            "case {case:?}: the wording must be built from the permitted-status list"
        );
    }

    // The positive control: from the first of the permitted statuses the reboot is accepted
    // and the instance reports the momentary outage.
    assert_eq!(
        REBOOTABLE_DB_INSTANCE_STATUSES[0], "available",
        "the first permitted status is the running one"
    );
    expect_ok(
        "a reboot of an available instance",
        call(
            &router,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "emu-reboot-available"),
                ("Engine", "postgres"),
                ("DBInstanceClass", "db.t3.micro"),
            ],
        )
        .await,
    );
    wait_for_settle();
    let rebooted = expect_ok(
        "a reboot of an available instance",
        call(
            &router,
            "RebootDBInstance",
            &[("DBInstanceIdentifier", "emu-reboot-available")],
        )
        .await,
    );
    assert_eq!(
        text(&rebooted, "DBInstanceStatus"),
        "rebooting",
        "case \"a reboot of an available instance\": expected the momentary outage, got \
         {rebooted}"
    );
}

// ---------------------------------------------------------------------------
// 8. An empty required description
// ---------------------------------------------------------------------------

/// One row of the empty-description table: the create, the member that carries the
/// description, and the other members that create needs.
struct BlankRow {
    action: &'static str,
    member: &'static str,
    rest: &'static [(&'static str, &'static str)],
    /// The describe that reads the created resource back, and the member it is named by.
    describe: &'static str,
    name_member: &'static str,
}

/// Every create whose model marks a description REQUIRED, so the rule is asserted across the
/// resource families rather than on the one the report cited.
const BLANK_ROWS: &[BlankRow] = &[
    BlankRow {
        action: "CreateDBParameterGroup",
        member: "Description",
        rest: &[("DBParameterGroupFamily", "mysql8.4")],
        describe: "DescribeDBParameterGroups",
        name_member: "DBParameterGroupName",
    },
    BlankRow {
        action: "CreateDBClusterParameterGroup",
        member: "Description",
        rest: &[("DBParameterGroupFamily", "aurora-mysql8.0")],
        describe: "DescribeDBClusterParameterGroups",
        name_member: "DBClusterParameterGroupName",
    },
    BlankRow {
        action: "CreateDBSecurityGroup",
        member: "DBSecurityGroupDescription",
        rest: &[],
        describe: "DescribeDBSecurityGroups",
        name_member: "DBSecurityGroupName",
    },
    BlankRow {
        action: "CreateDBSubnetGroup",
        member: "DBSubnetGroupDescription",
        rest: &[
            ("SubnetIds.SubnetIdentifier.1", "subnet-0a1b2c3d"),
            ("SubnetIds.SubnetIdentifier.2", "subnet-1a2b3c4d"),
        ],
        describe: "DescribeDBSubnetGroups",
        name_member: "DBSubnetGroupName",
    },
    BlankRow {
        action: "CreateOptionGroup",
        member: "OptionGroupDescription",
        rest: &[("EngineName", "mysql"), ("MajorEngineVersion", "8.0")],
        describe: "DescribeOptionGroups",
        name_member: "OptionGroupName",
    },
];

/// A required description that is present but empty is refused, and nothing is stored; the
/// same create with a description is served and reads the description back.
#[tokio::test]
async fn an_empty_required_description_is_refused_rather_than_stored() {
    let router = emulator();

    for (row_index, row) in BLANK_ROWS.iter().enumerate() {
        let name = format!("emu-empty-desc-{row_index}");
        let case = format!("{} with an empty {}", row.action, row.member);
        let mut params = vec![
            (row.name_member, name.clone()),
            (row.member, String::new()),
        ];
        params.extend(
            row.rest
                .iter()
                .map(|(member, value)| (*member, (*value).to_string())),
        );
        let body = expect_error(
            &case,
            call(&router, row.action, &params).await,
            400,
            INVALID_PARAMETER_VALUE,
        );
        let expected = format!(
            "The parameter {} must be provided and must not be blank.",
            row.member
        );
        let message = text(&body, "Message");
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );

        // Nothing was stored: the name the refused create carried resolves no resource.
        let (status, read_back) = call(
            &router,
            row.describe,
            &[(row.name_member, name.as_str())],
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused create must leave nothing behind, got {read_back}"
        );

        // The same create with a description is served, and the description round-trips.
        let described = "a description the create keeps";
        let mut served = vec![
            (row.name_member, name.clone()),
            (row.member, described.to_string()),
        ];
        served.extend(
            row.rest
                .iter()
                .map(|(member, value)| (*member, (*value).to_string())),
        );
        expect_ok(&case, call(&router, row.action, &served).await);
        let read_back = expect_ok(
            &case,
            call(
                &router,
                row.describe,
                &[(row.name_member, name.as_str())],
            )
            .await,
        );
        assert!(
            read_back.contains(described),
            "case {case:?}: expected the stored description to be reported, got {read_back}"
        );
    }
}

/// The emptiness rule itself: the empty string is the one value a required member cannot
/// carry, and every value with content — whitespace included, which is a value the caller
/// chose — is accepted. Asserted on the rule directly, because a raw whitespace-only value
/// cannot be carried in the form body this harness writes.
#[test]
fn only_an_empty_required_value_is_refused() {
    let refused = check_not_empty("Description", "");
    assert_eq!(
        refused,
        Err("The parameter Description must be provided and must not be blank.".to_string()),
        "case \"\": expected the empty-value refusal, got {refused:?}"
    );
    for value in [" ", "   ", "\t", "a", " a ", "a description", "0", "-"] {
        let accepted = check_not_empty("Description", value);
        assert!(
            accepted.is_ok(),
            "case {value:?}: a value with content must be accepted, got {accepted:?}"
        );
    }
}
