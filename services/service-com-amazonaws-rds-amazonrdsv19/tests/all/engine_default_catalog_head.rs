// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the HEAD of the engine-default parameter catalog
//! `DescribeEngineDefaultParameters` publishes for a MySQL-compatible DB parameter group
//! family.
//!
//! Every call goes over the wire through `db_instances::emulator`, the one mounting helper
//! this test target has, as a real awsQuery form POST carrying the SigV4 credential scope
//! the router keys on, so every assertion reads the XML the emulator itself produced.
//!
//! The rule under test: the report is the named family's engine's own published default
//! parameter set, in ascending `ParameterName` order, each row carrying that parameter's own
//! published record — so the FIRST page is the first parameters of that set rather than a
//! sample taken from the middle of it, and `EngineDefaults.Marker` is a position in that
//! same order, from which the next page continues the sequence.
//!
//! The expected rows below are LITERALS, written from the engines' own published parameter
//! data (the MySQL manual's "Server System Variables", which states each variable's apply
//! time, data type, domain, default and the release it was introduced in, and the Amazon RDS
//! User Guide's MySQL parameter list, which states which of them the managed engine lets a
//! caller modify), so a drift in this service's catalog fails this test instead of moving
//! with it. The table spans the families that share the MySQL vocabulary — MySQL itself,
//! MariaDB and Aurora MySQL, and two families of one engine — and the named-row table below
//! pins entries no issue cited.

use service_com_amazonaws_rds_amazonrdsv19::state::{
    APPLY_TYPE_DYNAMIC, APPLY_TYPE_STATIC, PARAMETER_SOURCE_ENGINE_DEFAULT,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{call, emulator, expect_ok, optional_text};
use crate::engine_default_parameters::{reported, Reported};

/// The operation under test.
const ACTION: &str = "DescribeEngineDefaultParameters";

/// The smallest page `MaxRecords` admits ("Constraints: Minimum 20, maximum 100"), which is
/// the window the head below is read through.
const PAGE: usize = 20;

/// The families that share the MySQL parameter vocabulary: the engine's own families, the
/// MariaDB and Aurora MySQL engines that run the same vocabulary, and a second family of one
/// engine — a family is one major version of an engine and the parameters belong to the
/// ENGINE, so every row must publish the same head.
const MYSQL_FAMILIES: [&str; 5] = [
    "mysql5.7",
    "mysql8.0",
    "aurora-mysql5.7",
    "aurora-mysql8.0",
    "mariadb10.11",
];

/// The first page of the MySQL vocabulary's published defaults, in the ascending
/// `ParameterName` order the report is paginated in.
const HEAD: [&str; PAGE] = [
    "allow-suspicious-udfs",
    "auto_generate_certs",
    "auto_increment_increment",
    "auto_increment_offset",
    "autocommit",
    "automatic_sp_privileges",
    "avoid_temporal_upgrade",
    "back_log",
    "basedir",
    "big_tables",
    "binlog_cache_size",
    "binlog_checksum",
    "binlog_direct_non_transactional_updates",
    "binlog_error_action",
    "binlog_format",
    "binlog_group_commit_sync_delay",
    "binlog_group_commit_sync_no_delay_count",
    "binlog_max_flush_queue_time",
    "binlog_order_commits",
    "binlog_row_image",
];

/// The parameter that follows [`HEAD`], so the page after the first page's `Marker` is
/// asserted to continue the one ordered sequence.
const AFTER_HEAD: &str = "binlog_rows_query_log_events";

/// The parameters of the first page the engine introduced INSIDE the releases these families
/// run, each with the release that introduced it: the only rows of the page that report a
/// `MinimumEngineVersion` at all, since "the earliest engine version to which the parameter
/// can apply" is only a fact worth reporting where the engine did not always carry the
/// parameter.
const INTRODUCED_IN: [(&str, &str); 3] = [
    ("auto_generate_certs", "5.7.5"),
    ("binlog_group_commit_sync_delay", "5.7.5"),
    ("binlog_group_commit_sync_no_delay_count", "5.7.5"),
];

/// One named row of the published record table: the parameter, its apply type, data type,
/// published domain (`None` where the engine bounds the value by nothing and publishes none),
/// default value (`None` where the engine publishes none) and whether a caller may modify it.
type Declared = (
    &'static str,
    &'static str,
    &'static str,
    Option<&'static str>,
    Option<&'static str>,
    bool,
);

/// Named rows the issue that filed this suite did not cite, spanning the record forms the
/// catalog carries: a parameter the managed engine does not let a caller modify, an
/// enumerated domain and a numeric range, a parameter the engine autosizes and so publishes
/// no default for, and both apply types.
const DECLARED: [Declared; 7] = [
    (
        "allow-suspicious-udfs",
        APPLY_TYPE_STATIC,
        "boolean",
        Some("0,1"),
        Some("0"),
        false,
    ),
    (
        "auto_increment_increment",
        APPLY_TYPE_DYNAMIC,
        "integer",
        Some("1-65535"),
        Some("1"),
        true,
    ),
    (
        "back_log",
        APPLY_TYPE_STATIC,
        "integer",
        Some("1-65535"),
        None,
        true,
    ),
    (
        // The installation directory the managed engine owns: it publishes neither a domain
        // nor a value for it, and does not let a caller modify it.
        "basedir",
        APPLY_TYPE_STATIC,
        "string",
        None,
        None,
        false,
    ),
    (
        "binlog_checksum",
        APPLY_TYPE_DYNAMIC,
        "string",
        Some("NONE,CRC32"),
        Some("CRC32"),
        true,
    ),
    (
        "binlog_format",
        APPLY_TYPE_DYNAMIC,
        "string",
        Some("ROW,STATEMENT,MIXED"),
        Some("MIXED"),
        true,
    ),
    (
        "binlog_row_image",
        APPLY_TYPE_DYNAMIC,
        "string",
        Some("full,minimal,noblob"),
        Some("full"),
        true,
    ),
];

/// One page of a family's defaults: the parameters and the `Marker` the answer carried.
async fn page(
    router: &MultiServiceRouter,
    case: &str,
    family: &str,
    marker: Option<&str>,
) -> (Vec<Reported>, Option<String>) {
    let mut parameters = vec![
        ("DBParameterGroupFamily", family.to_string()),
        ("MaxRecords", PAGE.to_string()),
    ];
    if let Some(marker) = marker {
        parameters.push(("Marker", marker.to_string()));
    }
    let body = expect_ok(case, call(router, ACTION, &parameters).await);
    (reported(&body), optional_text(&body, "Marker"))
}

/// The names of a page, in the order they were reported.
fn names(rows: &[Reported]) -> Vec<&str> {
    rows.iter().map(|row| row.name.as_str()).collect()
}

/// The first page of every MySQL-vocabulary family is the first parameters of that engine's
/// published default set, in ascending name order.
#[tokio::test]
async fn the_first_page_is_the_engines_first_published_defaults() {
    let router = emulator();

    for family in MYSQL_FAMILIES {
        let case = format!("family {family}");
        let (rows, marker) = page(&router, &case, family, None).await;
        assert_eq!(
            names(&rows),
            HEAD.to_vec(),
            "case {case:?}: expected the engine's first published defaults, got {:?}",
            names(&rows)
        );
        assert!(
            marker.is_some(),
            "case {case:?}: parameters are left after the first page, so a Marker is owed, \
             got {marker:?}"
        );

        // The page continues from the marker in the SAME order: the row after the page is
        // the next published parameter, and the two pages together stay strictly ascending.
        let (next, _) = page(&router, &case, family, marker.as_deref()).await;
        assert_eq!(
            names(&next).first().copied(),
            Some(AFTER_HEAD),
            "case {case:?}: the page after the Marker continues the published order, got {:?}",
            names(&next)
        );
        let walked: Vec<&str> = names(&rows).into_iter().chain(names(&next)).collect();
        let mut ordered = walked.clone();
        ordered.sort_unstable();
        ordered.dedup();
        assert_eq!(
            walked, ordered,
            "case {case:?}: the walk over both pages must be ascending and report no \
             parameter twice, got {walked:?}"
        );

        // Every row of the page is the engine's own default, and only the parameters the
        // engine introduced inside the releases these families run report the release that
        // introduced them.
        for row in &rows {
            let expected = INTRODUCED_IN
                .iter()
                .find(|(name, _)| *name == row.name)
                .map(|(_, version)| (*version).to_string());
            assert_eq!(
                row.minimum_engine_version, expected,
                "case {case:?}: {} must report MinimumEngineVersion {expected:?}, got {:?}",
                row.name, row.minimum_engine_version
            );
            assert_eq!(
                row.source.as_deref(),
                Some(PARAMETER_SOURCE_ENGINE_DEFAULT),
                "case {case:?}: {} is an engine default, got Source {:?}",
                row.name,
                row.source
            );
        }
    }
}

/// Each named parameter reports the whole record its engine publishes for it, and a
/// parameter the engine autosizes reports no `ParameterValue` at all rather than an empty
/// one.
#[tokio::test]
async fn each_published_parameter_reports_its_own_declared_record() {
    let router = emulator();

    for family in MYSQL_FAMILIES {
        let case = format!("family {family}");
        let (rows, _) = page(&router, &case, family, None).await;
        for (name, apply_type, data_type, allowed_values, value, is_modifiable) in DECLARED {
            let row = rows
                .iter()
                .find(|row| row.name == name)
                .unwrap_or_else(|| panic!("case {case:?}: expected {name} on the first page"));
            let found = (
                row.apply_type.as_deref(),
                row.data_type.as_deref(),
                row.allowed_values.as_deref(),
                row.value.as_deref(),
                row.is_modifiable.as_deref(),
            );
            let expected = (
                Some(apply_type),
                Some(data_type),
                allowed_values,
                value,
                Some(if is_modifiable { "true" } else { "false" }),
            );
            assert_eq!(
                found, expected,
                "case {case:?}: {name} must report the record its engine publishes, got \
                 {found:?}"
            );
            assert!(
                row.description.as_ref().is_some_and(|text| !text.is_empty()),
                "case {case:?}: {name} must report its Description, got {:?}",
                row.description
            );
        }
    }
}
