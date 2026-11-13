// SPDX-License-Identifier: CC0-1.0

//! What a drift check reports about the property a declaration states the resource's own
//! IDENTITY in, end to end over the wire.
//!
//! **The rule under test.** A resource's actual state is read from the service that OWNS it,
//! addressed by the resource's physical id, so a read that answered resolved that very
//! identifier to the resource it reported: the declared property holding that identifier
//! names the resource that answered, whichever of the forms it resolves the owning service
//! renders the identifier back in. A drift check therefore reports a difference only for the
//! properties that genuinely differ, never for the identity — and a resource the identifier
//! no longer resolves to is reported `DELETED` by the read itself rather than as a renamed
//! resource.
//!
//! The rows span what the rule generalises over rather than one recorded case: the name
//! FORMS one owning service resolves to the same resource (a top-level name stated with the
//! leading hierarchy separator, the same kind of name stated without it, and a name in a
//! hierarchy), a resource nothing touched beside ones changed out of band, and a second
//! owning service with a name property of its own spelling. Both drift views a client has
//! are asserted: the detection it polls — over a `LogicalResourceIds` subset and over the
//! whole stack — and the drift-aware change set that previews putting the template's values
//! back.
//!
//! Everything goes over the wire each service speaks — awsQuery for CloudFormation, REST for
//! Amazon S3, awsJson1_1 for AWS Systems Manager — through the one router `harness.rs`
//! builds.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::change_set_lifecycle::{create_change_set, describe_change_set, settle_the_store};
use crate::dynamic_references::ssm;
use crate::provisioned_resources::s3;
use crate::stack_lifecycle::{
    call, create_stack, list_members, member, router_with_engine, xml_blocks, xml_text,
};
use service_com_amazonaws_cloudformation_cloudformation::state::{
    drift::difference_type, drift_detection_status, resource_drift_status,
};

/// One difference a record must carry: the path, the expected value, the actual value and
/// the kind of difference.
type Difference = (&'static str, &'static str, &'static str, &'static str);

/// What a row does to its resource, outside the stack, before the check runs.
#[derive(Debug, Clone, Copy)]
enum Tamper {
    /// Nothing: the resource is as the deployment left it.
    Nothing,
    /// Write this value over the parameter's, through Parameter Store's own wire.
    WriteParameterValue(&'static str),
    /// Suspend the versioning the template declared for this bucket.
    SuspendVersioning,
}

/// One resource of the stack the table deploys, and what the check over it must report.
struct Row {
    case: &'static str,
    logical_id: &'static str,
    /// The resource declaration, as the template carries it.
    declaration: Value,
    tamper: Tamper,
    status: &'static str,
    differences: Vec<Difference>,
}

/// The `AWS::SSM::Parameter` declaration a row deploys: a parameter of Parameter Store under
/// the name the template asks for, which is this resource's physical id.
fn parameter(name: &str, value: &str) -> Value {
    json!({
        "Type": "AWS::SSM::Parameter",
        "Properties": {"Name": name, "Type": "String", "Value": value},
    })
}

/// The `AWS::S3::Bucket` declaration a row deploys, named by the property that type spells
/// its name in, with a nested configuration document of its own.
fn bucket(name: &str) -> Value {
    json!({
        "Type": "AWS::S3::Bucket",
        "Properties": {
            "BucketName": name,
            "VersioningConfiguration": {"Status": "Enabled"},
        },
    })
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a top-level parameter name declared with the leading separator",
            logical_id: "TopLevel",
            declaration: parameter("/drift-identity-top-level", "orig-top"),
            tamper: Tamper::WriteParameterValue("changed-top"),
            status: resource_drift_status::MODIFIED,
            differences: vec![(
                "/Value",
                "orig-top",
                "changed-top",
                difference_type::NOT_EQUAL,
            )],
        },
        Row {
            case: "the same kind of name declared without the leading separator",
            logical_id: "BareName",
            declaration: parameter("drift-identity-bare", "orig-bare"),
            tamper: Tamper::WriteParameterValue("changed-bare"),
            status: resource_drift_status::MODIFIED,
            differences: vec![(
                "/Value",
                "orig-bare",
                "changed-bare",
                difference_type::NOT_EQUAL,
            )],
        },
        Row {
            case: "a parameter name in a hierarchy",
            logical_id: "Hierarchy",
            declaration: parameter("/drift/identity/nested", "orig-nested"),
            tamper: Tamper::WriteParameterValue("changed-nested"),
            status: resource_drift_status::MODIFIED,
            differences: vec![(
                "/Value",
                "orig-nested",
                "changed-nested",
                difference_type::NOT_EQUAL,
            )],
        },
        Row {
            case: "a top-level parameter name nothing touched",
            logical_id: "Untouched",
            declaration: parameter("/drift-identity-untouched", "orig-untouched"),
            tamper: Tamper::Nothing,
            status: resource_drift_status::IN_SYNC,
            differences: vec![],
        },
        Row {
            case: "another owning service's named resource, changed inside its configuration",
            logical_id: "Store",
            declaration: bucket("drift-identity-store"),
            tamper: Tamper::SuspendVersioning,
            status: resource_drift_status::MODIFIED,
            differences: vec![(
                "/VersioningConfiguration/Status",
                "Enabled",
                "Suspended",
                difference_type::NOT_EQUAL,
            )],
        },
        Row {
            case: "another owning service's named resource nothing touched",
            logical_id: "Kept",
            declaration: bucket("drift-identity-kept"),
            tamper: Tamper::Nothing,
            status: resource_drift_status::IN_SYNC,
            differences: vec![],
        },
    ]
}

/// How long a case waits for a drift detection to CONCLUDE, and how often it looks again.
///
/// The wait is on the CONDITION — the detection reporting a terminal status — never on the
/// clock: the deadline is generous enough that a loaded machine still reaches it, and what
/// the case asserts is what the detection found.
const DETECTION_DEADLINE: Duration = Duration::from_secs(30);
const DETECTION_POLL: Duration = Duration::from_millis(25);

/// The physical id the stack reports for one of its resources — the identifier its actual
/// state is read back by.
async fn physical_id(router: &MultiServiceRouter, stack: &str, logical_id: &str) -> String {
    let (status, body) = call(
        router,
        "DescribeStackResource",
        &[
            member("StackName", stack),
            member("LogicalResourceId", logical_id),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeStackResource({stack}, {logical_id}) answered {status} with {body}"
    );
    xml_text(&body, "PhysicalResourceId")
        .unwrap_or_else(|| panic!("case {logical_id:?}: expected a physical id, got {body}"))
}

/// Carry out one row's out-of-band change, through the owning service's own wire.
async fn tamper_with(router: &MultiServiceRouter, stack: &str, row: &Row) {
    match row.tamper {
        Tamper::Nothing => {}
        Tamper::WriteParameterValue(value) => {
            let name = physical_id(router, stack, row.logical_id).await;
            let (status, body) = ssm(
                router,
                "PutParameter",
                &json!({"Name": name, "Type": "String", "Value": value, "Overwrite": true}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: writing {name:?} outside the stack answered {status} with {body}",
                row.case
            );
        }
        Tamper::SuspendVersioning => {
            let name = physical_id(router, stack, row.logical_id).await;
            let document = b"<VersioningConfiguration><Status>Suspended</Status>\
                             </VersioningConfiguration>";
            let (status, _, body) =
                s3(router, "PUT", &format!("/{name}?versioning"), document).await;
            assert!(
                (200..300).contains(&status),
                "case {:?}: suspending versioning on {name:?} answered {status} with {body}",
                row.case
            );
        }
    }
}

/// Start a drift detection over a stack, optionally over the logical ids named, and answer
/// the detection id it hands back.
async fn start_detection(
    router: &MultiServiceRouter,
    stack: &str,
    logical_ids: &[&str],
) -> String {
    let mut members = vec![member("StackName", stack)];
    for (index, logical_id) in logical_ids.iter().enumerate() {
        members.push(member(
            &format!("LogicalResourceIds.member.{}", index + 1),
            logical_id,
        ));
    }
    let (status, body) = call(router, "DetectStackDrift", &members).await;
    assert_eq!(
        status, 200,
        "DetectStackDrift({stack}, {logical_ids:?}) answered {status} with {body}"
    );
    xml_text(&body, "StackDriftDetectionId")
        .unwrap_or_else(|| panic!("expected a detection id, got {body}"))
}

/// Look again until the detection reports a terminal status, and assert it COMPLETED.
async fn await_the_detection(router: &MultiServiceRouter, detection_id: &str) {
    let deadline = Instant::now() + DETECTION_DEADLINE;
    loop {
        let (status, body) = call(
            router,
            "DescribeStackDriftDetectionStatus",
            &[member("StackDriftDetectionId", detection_id)],
        )
        .await;
        assert_eq!(
            status, 200,
            "DescribeStackDriftDetectionStatus({detection_id}) answered {status} with {body}"
        );
        let detection_status = xml_text(&body, "DetectionStatus").unwrap_or_default();
        if detection_status != drift_detection_status::DETECTION_IN_PROGRESS {
            assert_eq!(
                detection_status,
                drift_detection_status::DETECTION_COMPLETE,
                "expected the detection to complete, got {body}"
            );
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the detection {detection_id} never concluded: {body}"
        );
        tokio::time::sleep(DETECTION_POLL).await;
    }
}

/// What a record carries: its drift status, the differences it reports, and the properties
/// document each side was compared from.
type Record = (
    String,
    Vec<ReportedDifference>,
    Option<String>,
    Option<String>,
);

/// One reported difference, as the response spells it.
type ReportedDifference = (String, String, String, String);

/// Every drift record the stack reports, keyed by logical id.
async fn reported_records(router: &MultiServiceRouter, stack: &str) -> BTreeMap<String, Record> {
    let (status, body) = call(
        router,
        "DescribeStackResourceDrifts",
        &[member("StackName", stack)],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeStackResourceDrifts({stack}) answered {status} with {body}"
    );
    list_members(&body, "StackResourceDrifts")
        .into_iter()
        .map(|block| {
            let logical_id = xml_text(&block, "LogicalResourceId").unwrap_or_default();
            let record = (
                xml_text(&block, "StackResourceDriftStatus").unwrap_or_default(),
                differences_of(&block),
                xml_text(&block, "ExpectedProperties"),
                xml_text(&block, "ActualProperties"),
            );
            (logical_id, record)
        })
        .collect()
}

/// The differences one record carries, in wire order.
fn differences_of(block: &str) -> Vec<ReportedDifference> {
    list_members(block, "PropertyDifferences")
        .into_iter()
        .map(|difference| {
            (
                xml_text(&difference, "PropertyPath").unwrap_or_default(),
                xml_text(&difference, "ExpectedValue").unwrap_or_default(),
                xml_text(&difference, "ActualValue").unwrap_or_default(),
                xml_text(&difference, "DifferenceType").unwrap_or_default(),
            )
        })
        .collect()
}

/// A row's expected differences, as the reader renders them.
fn wanted(row: &Row) -> Vec<ReportedDifference> {
    row.differences
        .iter()
        .map(|(path, expected, actual, kind)| {
            (
                (*path).to_string(),
                (*expected).to_string(),
                (*actual).to_string(),
                (*kind).to_string(),
            )
        })
        .collect()
}

/// Assert one row's record: the drift status it adds up to and the differences it reports.
fn assert_record(row: &Row, records: &BTreeMap<String, Record>, view: &str) {
    let record = records.get(row.logical_id).unwrap_or_else(|| {
        panic!(
            "case {:?}: expected {} to report {:?}, got {:?}",
            row.case, view, row.logical_id, records
        )
    });
    let (status, differences, expected_properties, actual_properties) = record;
    assert_eq!(
        status, row.status,
        "case {:?}: expected {} to report {}, got {status} with {differences:?}",
        row.case, view, row.status
    );
    let want = wanted(row);
    assert_eq!(
        differences, &want,
        "case {:?}: expected {} to report the differences {want:?}, got {differences:?}",
        row.case, view
    );
    // The DECLARATION is reported in full whatever the comparison established about it, so
    // the identity the template stated is still there to read.
    let declared = row
        .declaration
        .pointer("/Properties")
        .expect("the row's declared properties");
    let text = expected_properties.clone().unwrap_or_else(|| {
        panic!(
            "case {:?}: expected {} to report the expected properties",
            row.case, view
        )
    });
    let reported: Value = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("case {:?}: {text:?} is not a document: {error}", row.case));
    assert_eq!(
        &reported, declared,
        "case {:?}: expected {} to report the declaration {declared}, got {reported}",
        row.case, view
    );
    // A resource that WAS compared reports what the owning service answered with, whatever
    // the comparison made of it.
    assert!(
        actual_properties.is_some(),
        "case {:?}: expected {} to report the configuration it compared against, got {record:?}",
        row.case,
        view
    );
}

/// RULE: a drift check reports a difference only for the properties that genuinely differ —
/// never for the property the declaration states the resource's own identity in, which the
/// read that reported the resource was addressed by.
#[tokio::test]
async fn the_identity_a_resource_is_read_back_by_is_never_reported_as_drifted() {
    let (router, engine) = router_with_engine();
    let stack = "drift-identity";
    let rows = rows();
    let resources: Value = rows
        .iter()
        .map(|row| (row.logical_id.to_string(), row.declaration.clone()))
        .collect::<serde_json::Map<String, Value>>()
        .into();
    let template = json!({"Resources": resources}).to_string();

    let (status, body) = create_stack(&router, stack, &template, &[]).await;
    assert_eq!(status, 200, "the stack must be created, got {status} {body}");
    settle_the_store(&router, &engine).await;

    for row in &rows {
        tamper_with(&router, stack, row).await;
    }

    // A detection over a SUBSET of the stack's resources checks only the resources it names.
    let subset: Vec<&str> = vec!["TopLevel", "Untouched"];
    let detection = start_detection(&router, stack, &subset).await;
    await_the_detection(&router, &detection).await;
    let records = reported_records(&router, stack).await;
    let checked: Vec<&String> = records.keys().collect();
    let mut expected_checked = subset.clone();
    expected_checked.sort_unstable();
    assert_eq!(
        checked,
        expected_checked.iter().collect::<Vec<&&str>>(),
        "expected only the named resources to be checked, got {checked:?}"
    );
    for row in rows.iter().filter(|row| subset.contains(&row.logical_id)) {
        assert_record(row, &records, "the subset detection");
    }

    // ...and a detection over the whole stack reports every one of them the same way.
    let detection = start_detection(&router, stack, &[]).await;
    await_the_detection(&router, &detection).await;
    let records = reported_records(&router, stack).await;
    for row in &rows {
        assert_record(row, &records, "the whole-stack detection");
    }
}

/// RULE: the same comparison stands behind a drift-aware change set, so the targets it
/// previews are the properties that genuinely differ — the resource's identity is not one of
/// them, and a resource whose only difference would have been its identity is no change at
/// all.
#[tokio::test]
async fn a_revert_drift_change_set_targets_only_the_properties_that_differ() {
    let (router, engine) = router_with_engine();

    /// `(case, stack, the parameter's declared name, the value written out of band)`.
    type Case = (&'static str, &'static str, &'static str, Option<&'static str>);
    let cases: Vec<Case> = vec![
        (
            "a top-level name declared with the leading separator, value changed",
            "revert-identity-top",
            "/revert-identity-top",
            Some("drifted"),
        ),
        (
            "a name in a hierarchy, value changed",
            "revert-identity-deep",
            "/revert/identity/deep",
            Some("drifted"),
        ),
        (
            "a top-level name declared with the leading separator, nothing changed",
            "revert-identity-clean",
            "/revert-identity-clean",
            None,
        ),
    ];

    for (case, stack, name, live) in &cases {
        let declared = "declared";
        let template = json!({"Resources": {"Param": parameter(name, declared)}}).to_string();
        let (status, body) = create_stack(&router, stack, &template, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the stack must be created, got {status} {body}"
        );
        settle_the_store(&router, &engine).await;

        if let Some(live) = live {
            let (status, body) = ssm(
                &router,
                "PutParameter",
                &json!({"Name": name, "Type": "String", "Value": live, "Overwrite": true}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the out-of-band write must land, got {status} {body}"
            );
        }

        let (status, body) = create_change_set(
            &router,
            stack,
            "cs-revert",
            &[
                member("UsePreviousTemplate", "true"),
                member("DeploymentMode", "REVERT_DRIFT"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the drift-aware change set must be created, got {status} {body}"
        );
        settle_the_store(&router, &engine).await;
        let (_, body) = describe_change_set(&router, "cs-revert", Some(stack)).await;

        let targets: Vec<(String, String, String, String, String)> = xml_blocks(&body, "Target")
            .into_iter()
            .map(|target| {
                (
                    xml_text(&target, "Name").unwrap_or_default(),
                    xml_text(&target, "BeforeValue").unwrap_or_default(),
                    xml_text(&target, "BeforeValueFrom").unwrap_or_default(),
                    xml_text(&target, "AfterValue").unwrap_or_default(),
                    xml_text(&target, "AfterValueFrom").unwrap_or_default(),
                )
            })
            .collect();
        let want: Vec<(String, String, String, String, String)> = live
            .map(|live| {
                vec![(
                    "Value".to_string(),
                    live.to_string(),
                    "ACTUAL_STATE".to_string(),
                    declared.to_string(),
                    "TEMPLATE".to_string(),
                )]
            })
            .unwrap_or_default();
        assert_eq!(
            targets, want,
            "case {case:?}: expected the targets {want:?}, got {targets:?} from {body}"
        );
    }
}
