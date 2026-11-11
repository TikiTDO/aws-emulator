// SPDX-License-Identifier: CC0-1.0

//! `AWS::SSM::Association` end to end: the association a stack declares exists in AWS Systems
//! Manager itself, under the identifier THAT service issued.
//!
//! RULE: the physical id a stack publishes for a resource whose owning service MINTS the
//! identifier is the identifier that service answered its create with — so the value a `Ref`,
//! an `Fn::GetAtt` and `StackResource$PhysicalResourceId` carry is one that service accepts as
//! a handle, and deleting the stack releases the resource there. For a State Manager
//! association that identifier is the `AssociationId` `CreateAssociation` answers, never the
//! document `Name` the declaration states: Systems Manager refuses anything but the 8-4-4-4-12
//! hexadecimal its own `AssociationId` fixes.
//!
//! The rows differ along what the rule generalises over: the template spelling (JSON or YAML),
//! which document the association applies and whether it takes parameters, how it names the
//! nodes it applies to (the deprecated `InstanceId` or a `Targets` criterion), whether the
//! declaration carries an `AssociationName` of its own, and the schedule it is applied on.
//!
//! Every case drives the real request path for both services through one router — the
//! CloudFormation calls over the awsQuery wire and the Systems Manager calls over its
//! awsJson1_1 wire, each as a client's SDK makes them — so what is asserted is what a client
//! that deploys the template and then reads the association back observes.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::dynamodb_table_resources::failure_reasons;
use crate::dynamic_references::ssm;
use crate::nested_stacks::physical_resource_id;
use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, router, wait_for_the_transition, xml_blocks,
    xml_text,
};

/// The document the first row's association applies, and the one the second row's does.
///
/// Both are documents Systems Manager PUBLISHES, so the association resolves against that
/// service's own catalogue rather than against anything a test wrote: the first declares a
/// required `commands` parameter and the second declares none.
const RUN_SHELL_SCRIPT: &str = "AWS-RunShellScript";
const UPDATE_SSM_AGENT: &str = "AWS-UpdateSSMAgent";

/// A managed-node id of the form `InstanceId` takes
/// (`^(^i-(\w{8}|\w{17})$)|(^mi-\w{17}$)$`).
const NODE: &str = "i-0123456789abcdef0";

/// Whether `value` is an identifier of the form Systems Manager issues for an association:
/// the 8-4-4-4-12 hexadecimal groups its `AssociationId` pattern fixes
/// (`^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`), which is
/// what `DescribeAssociation` refuses anything else against.
fn is_association_id(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(length, group)| {
                group.len() == *length && group.chars().all(|digit| digit.is_ascii_hexdigit())
            })
}

/// Every output a `DescribeStacks` response reports, as the value under `key`.
fn output(xml: &str, key: &str) -> Option<String> {
    let block = xml_blocks(xml, "Outputs").into_iter().next()?;
    xml_blocks(&block, "member").into_iter().find_map(|entry| {
        (xml_text(&entry, "OutputKey").as_deref() == Some(key))
            .then(|| xml_text(&entry, "OutputValue"))
            .flatten()
    })
}

/// The association Systems Manager holds under `association_id`, or `None` where it holds
/// none — including where it refuses the value as no association id at all.
async fn described_association(
    router: &MultiServiceRouter,
    association_id: &str,
) -> Option<Value> {
    let (status, document) = ssm(
        router,
        "DescribeAssociation",
        &json!({"AssociationId": association_id}),
    )
    .await;
    if status != 200 {
        return None;
    }
    document.get("AssociationDescription").cloned()
}

/// One row of the table: a template, and what the rule says must then be in Systems Manager.
struct Row {
    case: &'static str,
    stack: &'static str,
    template: String,
    /// The document the association must report applying.
    document: &'static str,
    /// The members Systems Manager must report for the association, beside its id.
    reported: Value,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a JSON template applying a parameterised document to one node, on a rate \
                   schedule, under a name of its own",
            stack: "ssm-association-named",
            template: json!({
                "Resources": {
                    "Association": {
                        "Type": "AWS::SSM::Association",
                        "Properties": {
                            "Name": RUN_SHELL_SCRIPT,
                            "AssociationName": "nightly-shell",
                            "InstanceId": NODE,
                            "ScheduleExpression": "rate(30 minutes)",
                            "Parameters": {"commands": ["echo deployed"]}
                        }
                    }
                },
                "Outputs": {
                    "Reference": {"Value": {"Ref": "Association"}},
                    "Attribute": {
                        "Value": {"Fn::GetAtt": ["Association", "AssociationId"]}
                    }
                }
            })
            .to_string(),
            document: RUN_SHELL_SCRIPT,
            reported: json!({
                "AssociationName": "nightly-shell",
                "InstanceId": NODE,
                "ScheduleExpression": "rate(30 minutes)",
                "Parameters": {"commands": ["echo deployed"]}
            }),
        },
        Row {
            case: "a YAML template applying a parameterless document to a targets criterion, \
                   on a cron schedule, with no name of its own",
            stack: "ssm-association-targeted",
            template: [
                "Resources:",
                "  Association:",
                "    Type: AWS::SSM::Association",
                "    Properties:",
                "      Name: AWS-UpdateSSMAgent",
                "      ScheduleExpression: cron(0 2 ? * SUN *)",
                "      ApplyOnlyAtCronInterval: true",
                "      ComplianceSeverity: MEDIUM",
                "      MaxConcurrency: '1'",
                "      MaxErrors: '0'",
                "      Targets:",
                "        - Key: InstanceIds",
                "          Values:",
                "            - i-0123456789abcdef0",
                "Outputs:",
                "  Reference:",
                "    Value: !Ref Association",
                "  Attribute:",
                "    Value: !GetAtt Association.AssociationId",
            ]
            .join("\n"),
            document: UPDATE_SSM_AGENT,
            reported: json!({
                "ScheduleExpression": "cron(0 2 ? * SUN *)",
                "ApplyOnlyAtCronInterval": true,
                "ComplianceSeverity": "MEDIUM",
                "MaxConcurrency": "1",
                "MaxErrors": "0",
                "Targets": [{"Key": "InstanceIds", "Values": [NODE]}]
            }),
        },
    ]
}

/// RULE: every row's stack completes, publishes the `AssociationId` Systems Manager issued as
/// the association's `Ref`, its `AssociationId` attribute and its `PhysicalResourceId`, and
/// that service really holds the association the template describes under it — and the id is
/// never the document name the declaration states.
#[tokio::test]
async fn an_association_carries_the_identifier_systems_manager_issued() {
    for row in rows() {
        let case = row.case;
        let router = router();
        let (status, body) = create_stack(&router, row.stack, &row.template, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with {body}"
        );
        wait_for_the_transition().await;
        let (_, body) = describe_stack(&router, row.stack).await;
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to complete, got {body}"
        );

        let reference = output(&body, "Reference").unwrap_or_else(|| {
            panic!("case {case:?}: the stack published no Ref for the association: {body}")
        });
        let attribute = output(&body, "Attribute").unwrap_or_else(|| {
            panic!("case {case:?}: the stack published no AssociationId attribute: {body}")
        });
        assert_eq!(
            attribute, reference,
            "case {case:?}: expected Ref and the AssociationId attribute to be the one id, \
             got {reference:?} and {attribute:?}"
        );
        assert!(
            is_association_id(&reference),
            "case {case:?}: expected an id of the form Systems Manager issues, got \
             {reference:?}"
        );
        assert_ne!(
            reference, row.document,
            "case {case:?}: expected the association's id rather than the document it applies"
        );
        let recorded = physical_resource_id(case, &router, row.stack, "Association").await;
        assert_eq!(
            recorded, reference,
            "case {case:?}: expected the recorded PhysicalResourceId to be the published id, \
             got {recorded:?}"
        );

        // The association really exists in Systems Manager, under that id, as the template
        // describes it.
        let association = described_association(&router, &reference)
            .await
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: Systems Manager holds no association under the id the \
                     stack published ({reference:?})"
                )
            });
        assert_eq!(
            association.get("AssociationId").and_then(Value::as_str),
            Some(reference.as_str()),
            "case {case:?}: expected the association to report the published id, got \
             {association}"
        );
        assert_eq!(
            association.get("Name").and_then(Value::as_str),
            Some(row.document),
            "case {case:?}: expected the association to apply the declared document, got \
             {association}"
        );
        for (member, expected) in row
            .reported
            .as_object()
            .expect("a row's expected members are an object")
        {
            assert_eq!(
                association.get(member),
                Some(expected),
                "case {case:?}: expected the association's {member} to be {expected}, got \
                 {association}"
            );
        }

        // Deleting the stack releases the association in Systems Manager.
        let (status, body) = call(&router, "DeleteStack", &[member("StackName", row.stack)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with {body}"
        );
        wait_for_the_transition().await;
        // The teardown completed: a deleted stack either reports `DELETE_COMPLETE` or stops
        // resolving by name, and never `DELETE_FAILED` — the association's own delete is one
        // Systems Manager carries out.
        let (describe_status, described) = describe_stack(&router, row.stack).await;
        let reported = xml_text(&described, "StackStatus").unwrap_or_default();
        assert!(
            describe_status != 200 || reported == "DELETE_COMPLETE",
            "case {case:?}: expected the stack's teardown to complete, got {reported:?} with \
             {:?}",
            failure_reasons(&router, row.stack).await
        );
        let held = described_association(&router, &reference).await;
        assert!(
            held.is_none(),
            "case {case:?}: expected the deleted stack to have released the association, \
             Systems Manager still reports {held:?}"
        );
    }
}

/// RULE (the other half): a changed declaration reaches the association Systems Manager
/// already holds — the id it was created under is the handle `UpdateAssociation` takes — so
/// the stack keeps publishing that id and the service reports the new declaration.
#[tokio::test]
async fn a_changed_declaration_updates_the_association_in_place() {
    let router = router();
    let stack = "ssm-association-updated";
    let template = |schedule: &str, command: &str| {
        json!({
            "Resources": {
                "Association": {
                    "Type": "AWS::SSM::Association",
                    "Properties": {
                        "Name": RUN_SHELL_SCRIPT,
                        "AssociationName": "the-association",
                        "InstanceId": NODE,
                        "ScheduleExpression": schedule,
                        "Parameters": {"commands": [command]}
                    }
                }
            },
            "Outputs": {"Reference": {"Value": {"Ref": "Association"}}}
        })
        .to_string()
    };

    let case = "the stack that creates the association";
    let (status, body) = create_stack(
        &router,
        stack,
        &template("rate(30 minutes)", "echo first"),
        &[],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with {body}"
    );
    wait_for_the_transition().await;
    let (_, body) = describe_stack(&router, stack).await;
    let created = output(&body, "Reference")
        .unwrap_or_else(|| panic!("case {case:?}: the stack published no association id: {body}"));

    let case = "the update, which restates the schedule and the parameters";
    let (status, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", stack),
            member(
                "TemplateBody",
                &template("rate(2 hours)", "echo second"),
            ),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from UpdateStack, got {status} with {body}"
    );
    wait_for_the_transition().await;
    let (_, body) = describe_stack(&router, stack).await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("UPDATE_COMPLETE"),
        "case {case:?}: expected the update to complete, got {:?} with {:?}",
        xml_text(&body, "StackStatus"),
        failure_reasons(&router, stack).await
    );
    let updated = output(&body, "Reference")
        .unwrap_or_else(|| panic!("case {case:?}: the stack published no association id: {body}"));
    assert_eq!(
        updated, created,
        "case {case:?}: expected the association to keep the id it was created under, got \
         {updated:?} after {created:?}"
    );

    let association = described_association(&router, &updated)
        .await
        .unwrap_or_else(|| {
            panic!("case {case:?}: Systems Manager holds no association under {updated:?}")
        });
    assert_eq!(
        association.get("Parameters"),
        Some(&json!({"commands": ["echo second"]})),
        "case {case:?}: expected the restated parameters to reach the association, got \
         {association}"
    );
    assert_eq!(
        association
            .get("ScheduleExpression")
            .and_then(Value::as_str),
        Some("rate(2 hours)"),
        "case {case:?}: expected the restated schedule to reach the association, got \
         {association}"
    );
}
