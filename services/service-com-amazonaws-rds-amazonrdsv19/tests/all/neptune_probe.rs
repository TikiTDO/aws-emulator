// SPDX-License-Identifier: CC0-1.0

//! A probe of the Amazon Neptune control plane's parameter catalogs, which this one `rds`
//! front door serves (`src/service.rs`): an AWS SDK Neptune client signs `rds`, so its
//! `DescribeDBParameters` and `DescribeDBClusterParameters` are answered here.
//!
//! The rule under test: a parameter's published metadata belongs to the PARAMETER, so a
//! GROUP of a family reports the same apply type and description for one of its parameters
//! as the engine-default report of that same family and LEVEL does. `ApplyType` is what
//! tells a client whether the change it is about to make needs a reboot, so a group's report
//! and the engine's own disagreeing about it would answer that question differently
//! depending on which was asked.
//!
//! The two levels of this engine publish their own parameters — the graph query timeout
//! belongs to a DB parameter group, the audit log to a DB CLUSTER parameter group — so each
//! level is probed with a parameter that level publishes.
//!
//! Every call goes over the wire through the harness in `db_instances.rs`.

use crate::db_instances::{call, emulator, expect_ok, subtrees, text};

/// One level of the engine's catalog: (case, the group create, the member naming the group
/// on the create, the group's describe, the member naming the group on the describe, the
/// engine-default report of that level, the parameter that level publishes, the `ApplyType`
/// the engine publishes for it).
type Level = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
);

#[tokio::test]
async fn a_group_and_the_engine_default_report_agree_about_a_parameter() {
    let router = emulator();
    let family = "neptune1.4";

    let levels: [Level; 2] = [
        (
            "the DB parameter group's catalog",
            "CreateDBParameterGroup",
            "DBParameterGroupName",
            "DescribeDBParameters",
            "DBParameterGroupName",
            "DescribeEngineDefaultParameters",
            "neptune_query_timeout",
            // A change to the graph query timeout takes effect only once the instance is
            // rebooted, which is what `static` tells a caller.
            "static",
        ),
        (
            "the DB cluster parameter group's catalog",
            "CreateDBClusterParameterGroup",
            "DBClusterParameterGroupName",
            "DescribeDBClusterParameters",
            "DBClusterParameterGroupName",
            "DescribeEngineDefaultClusterParameters",
            "neptune_enable_audit_log",
            "static",
        ),
    ];

    for (
        case,
        create,
        create_member,
        describe,
        describe_member,
        engine_defaults,
        parameter,
        apply_type,
    ) in levels
    {
        let group = format!("emu-neptune-{}", parameter.replace('_', "-"));
        expect_ok(
            case,
            call(
                &router,
                create,
                &[
                    (create_member, group.as_str()),
                    ("DBParameterGroupFamily", family),
                    ("Description", "the engine's own metadata"),
                ],
            )
            .await,
        );

        // (where the parameter was read, its ApplyType, its Description)
        let mut reported: Vec<(String, String, String)> = Vec::new();
        for (source, action, params) in [
            (
                "the group's report",
                describe,
                vec![(describe_member, group.as_str())],
            ),
            (
                "the engine-default report",
                engine_defaults,
                vec![("DBParameterGroupFamily", family)],
            ),
        ] {
            let body = expect_ok(case, call(&router, action, &params).await);
            let parameters = subtrees(&body, "Parameter");
            let found = parameters
                .iter()
                .find(|candidate| text(candidate, "ParameterName") == parameter)
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: expected {source} of {family} to report {parameter}, \
                         got {body}"
                    )
                });
            reported.push((
                source.to_string(),
                text(found, "ApplyType"),
                text(found, "Description"),
            ));
        }

        for (source, reported_apply_type, description) in &reported {
            assert_eq!(
                reported_apply_type, apply_type,
                "case {case:?}: {source} must report ApplyType {apply_type:?} for \
                 {parameter}, got {reported_apply_type:?}"
            );
            assert!(
                !description.is_empty(),
                "case {case:?}: {source} must carry the engine's own description for \
                 {parameter}, got {description:?}"
            );
        }
        assert_eq!(
            (&reported[0].1, &reported[0].2),
            (&reported[1].1, &reported[1].2),
            "case {case:?}: the group's report and the engine-default report must agree \
             about {parameter}, got {reported:?}"
        );
    }
}
