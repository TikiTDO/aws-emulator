// SPDX-License-Identifier: CC0-1.0

//! The PIN an imported export puts on the stack publishing it, and the RELEASE of that pin,
//! end to end over the awsQuery wire.
//!
//! RULE under test: **an exported output value is immutable, in VALUE and in existence, for
//! as long as another live stack imports it — a deployment of the publishing stack that would
//! publish a different value under that name, or stop publishing it, is accepted and then
//! CANCELLED, leaving the stack in `UPDATE_ROLLBACK_COMPLETE` with a `StackStatusReason`
//! naming the export and the importing stack, and leaving the published value and the
//! importing stack's consuming property exactly as the importer resolved them — and the pin
//! is RELEASED the moment the last importing stack is gone, so the byte-identical deployment
//! then completes and publishes what it was asked to.**
//!
//! Why the release half matters as much as the refusal: a guard that never lets go is not the
//! cross-stack contract, it is a stack that can never be revised. A client whose pipeline is
//! blocked by this refusal removes the consumer and re-runs the identical deployment, and that
//! second attempt has to go through — otherwise the export is pinned by a stack that no longer
//! exists.
//!
//! `update_stack_export_in_use.rs` pins the refusal over several ways of withdrawing an
//! export and both calls that deploy a revision; this suite's rows differ along what the
//! RELEASE generalises over, and along the surfaces the pinned value has to survive on:
//! WHERE the exported value comes from (a template `Parameter` the revision re-values, a
//! literal the revision rewrites, an intrinsic composed over a parameter), HOW the importing
//! stack consumes the import (a resource property really written in the owning service, an
//! output value of its own), and WHETHER the revision changes the value or drops the export
//! outright. The recorded call this suite was written for is the first row: an export whose
//! value is a parameter, consumed by an `AWS::SNS::Topic` property in another stack.

use std::time::{Duration, Instant};

use service_com_amazonaws_cloudformation_cloudformation::state::status;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{list_imports_page, xml_blocks};
use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, router, sns, xml_text, xml_texts, REGION,
};

/// How often a waiting case looks again. Short enough that a case does not idle, and only
/// ever inside a loop that waits for the CONDITION.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// How long a case is willing to wait for a condition the service owes. Deliberately far
/// larger than any span this service schedules, so a loaded host does not decide the outcome.
const DEADLINE: Duration = Duration::from_secs(60);

/// The `StackStatusReason` CloudFormation records on a deployment it cancelled because the
/// deployment would have withdrawn an exported output value another stack still imports.
///
/// Spelled out here rather than read off the emulator's own formatter, so the assertion pins
/// the wording: fixed text around the export name and the importing stack's name, ending with
/// its period.
fn update_canceled_reason(export_name: &str, importing_stack_name: &str) -> String {
    format!(
        "Update canceled. Cannot update export {export_name} as it is in use by \
         {importing_stack_name}."
    )
}

/// How the importing stack consumes the value it imports — which is the surface the pinned
/// value has to survive on.
#[derive(Clone, Copy, Debug)]
enum Consumer {
    /// A property of a resource the stack really creates in the owning service, read back
    /// through that service's own wire.
    TopicDisplayName { logical_id: &'static str },
    /// An output of the importing stack's own.
    StackOutput { key: &'static str },
}

/// One row: a stack publishing one export, the revision it is asked to deploy, and the stack
/// that imports the export in the meantime.
struct Row {
    case: &'static str,
    exporter: &'static str,
    importer: &'static str,
    export_name: &'static str,
    /// The exporter's template as first deployed.
    template: &'static str,
    /// The template the revision deploys — the same text where the change is in the
    /// parameters alone.
    revised_template: &'static str,
    parameters: &'static [(&'static str, &'static str)],
    revised_parameters: &'static [(&'static str, &'static str)],
    /// The value the export publishes before the revision is attempted.
    published: &'static str,
    /// What the revision would publish under that name, or `None` where it DROPS the export.
    revised_publication: Option<&'static str>,
    importer_template: &'static str,
    consumer: Consumer,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the exported value comes from a Parameter the revision re-values, and the \
                   importer consumes it in a resource property",
            exporter: "param-exporter",
            importer: "param-importer",
            export_name: "param-sourced-export",
            template: r#"{"Parameters":{"Shared":{"Type":"String"}},
                 "Resources":{"Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}},
                 "Outputs":{"Exported":{"Value":{"Ref":"Shared"},
                   "Export":{"Name":"param-sourced-export"}}}}"#,
            revised_template: r#"{"Parameters":{"Shared":{"Type":"String"}},
                 "Resources":{"Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}},
                 "Outputs":{"Exported":{"Value":{"Ref":"Shared"},
                   "Export":{"Name":"param-sourced-export"}}}}"#,
            parameters: &[("Shared", "shared-first")],
            revised_parameters: &[("Shared", "shared-second")],
            published: "shared-first",
            revised_publication: Some("shared-second"),
            importer_template: r#"{"Resources":{"Consumer":{"Type":"AWS::SNS::Topic",
                 "Properties":{"DisplayName":{"Fn::ImportValue":"param-sourced-export"}}}}}"#,
            consumer: Consumer::TopicDisplayName {
                logical_id: "Consumer",
            },
        },
        Row {
            case: "the exported value is a literal the revision rewrites, and the importer \
                   echoes it in an output of its own",
            exporter: "literal-exporter",
            importer: "literal-importer",
            export_name: "literal-export",
            template: r#"{"Resources":{"Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}},
                 "Outputs":{"Exported":{"Value":"literal-first",
                   "Export":{"Name":"literal-export"}}}}"#,
            revised_template: r#"{"Resources":{"Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}},
                 "Outputs":{"Exported":{"Value":"literal-second",
                   "Export":{"Name":"literal-export"}}}}"#,
            parameters: &[],
            revised_parameters: &[],
            published: "literal-first",
            revised_publication: Some("literal-second"),
            importer_template: r#"{"Resources":{"Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}},
                 "Outputs":{"Echoed":{"Value":{"Fn::ImportValue":"literal-export"}}}}"#,
            consumer: Consumer::StackOutput { key: "Echoed" },
        },
        Row {
            case: "the exported value is composed by an intrinsic over a Parameter, and the \
                   importer consumes it in a resource property",
            exporter: "composed-exporter",
            importer: "composed-importer",
            export_name: "composed-export",
            template: r#"{"Parameters":{"Tail":{"Type":"String"}},
                 "Resources":{"Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}},
                 "Outputs":{"Exported":{"Value":{"Fn::Join":["-",["head",{"Ref":"Tail"}]]},
                   "Export":{"Name":"composed-export"}}}}"#,
            revised_template: r#"{"Parameters":{"Tail":{"Type":"String"}},
                 "Resources":{"Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}},
                 "Outputs":{"Exported":{"Value":{"Fn::Join":["-",["head",{"Ref":"Tail"}]]},
                   "Export":{"Name":"composed-export"}}}}"#,
            parameters: &[("Tail", "one")],
            revised_parameters: &[("Tail", "two")],
            published: "head-one",
            revised_publication: Some("head-two"),
            importer_template: r#"{"Resources":{"Consumer":{"Type":"AWS::SNS::Topic",
                 "Properties":{"DisplayName":{"Fn::ImportValue":"composed-export"}}}}}"#,
            consumer: Consumer::TopicDisplayName {
                logical_id: "Consumer",
            },
        },
        Row {
            case: "the revision DROPS the export instead of re-valuing it",
            exporter: "dropping-exporter",
            importer: "dropping-importer",
            export_name: "dropped-export",
            template: r#"{"Resources":{"Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}},
                 "Outputs":{"Exported":{"Value":"dropped-first",
                   "Export":{"Name":"dropped-export"}}}}"#,
            revised_template: r#"{"Resources":{"Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}},
                 "Outputs":{"Kept":{"Value":"kept-value"}}}"#,
            parameters: &[],
            revised_parameters: &[],
            published: "dropped-first",
            revised_publication: None,
            importer_template: r#"{"Resources":{"Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}},
                 "Outputs":{"Echoed":{"Value":{"Fn::ImportValue":"dropped-export"}}}}"#,
            consumer: Consumer::StackOutput { key: "Echoed" },
        },
    ]
}

/// The `Parameters` members of a request, as an SDK form-encodes them.
fn parameter_members(parameters: &[(&str, &str)]) -> Vec<(String, String)> {
    parameters
        .iter()
        .enumerate()
        .flat_map(|(index, (key, value))| {
            let position = index + 1;
            [
                member(&format!("Parameters.member.{position}.ParameterKey"), key),
                member(
                    &format!("Parameters.member.{position}.ParameterValue"),
                    value,
                ),
            ]
        })
        .collect()
}

/// The `(StackStatus, StackStatusReason)` one stack reports right now.
async fn reported_status(case: &str, router: &MultiServiceRouter, stack: &str) -> (String, String) {
    let (code, body) = describe_stack(router, stack).await;
    assert_eq!(
        code, 200,
        "case {case:?}: DescribeStacks({stack}) must answer 200, got {code} {body}"
    );
    (
        xml_text(&body, "StackStatus").unwrap_or_default(),
        xml_text(&body, "StackStatusReason").unwrap_or_default(),
    )
}

/// Poll `DescribeStacks` the way a client's waiter does and answer the
/// `(StackStatus, StackStatusReason)` the stack SETTLED on — the first status that is not an
/// operation in flight.
///
/// The condition is what is waited for, never a span: the loop looks again until the stack
/// has settled or `DEADLINE` has passed, and reports every status it saw on the way.
async fn settled_status(case: &str, router: &MultiServiceRouter, stack: &str) -> (String, String) {
    let deadline = Instant::now() + DEADLINE;
    let mut seen: Vec<String> = Vec::new();
    loop {
        let (reported, reason) = reported_status(case, router, stack).await;
        if !reported.is_empty() && !reported.ends_with("_IN_PROGRESS") {
            return (reported, reason);
        }
        if seen.last().map(String::as_str) != Some(reported.as_str()) {
            seen.push(reported);
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected {stack} to settle on a status that is not an operation in \
             flight, saw {seen:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The value `ListExports` publishes under `export_name`, or `None` where this Region
/// publishes no such export.
async fn published_value(
    case: &str,
    router: &MultiServiceRouter,
    export_name: &str,
) -> Option<String> {
    let (code, body) = call(router, "ListExports", &[]).await;
    assert_eq!(
        code, 200,
        "case {case:?}: ListExports must answer 200, got {code} {body}"
    );
    xml_texts(&body, "Name")
        .into_iter()
        .zip(xml_texts(&body, "Value"))
        .find(|(name, _)| name == export_name)
        .map(|(_, value)| value)
}

/// The stack names `ListImports` reports for `export_name` right now. An export nothing
/// imports — and a name nothing publishes — is REFUSED rather than answered with an empty
/// page (`unimported_exports.rs`), which this reads as the empty set: either way no live stack
/// holds the export.
async fn importing_stacks(router: &MultiServiceRouter, export_name: &str) -> Vec<String> {
    let (code, _, names, _) = list_imports_page(router, REGION, export_name, None).await;
    if code == 200 {
        names
    } else {
        Vec::new()
    }
}

/// The `PhysicalResourceId` the stack reports for one of its logical resources — for an
/// `AWS::SNS::Topic`, the ARN of the topic the stack really created in Amazon SNS.
async fn physical_id(
    case: &str,
    router: &MultiServiceRouter,
    stack: &str,
    logical_id: &str,
) -> String {
    let (code, body) = call(
        router,
        "DescribeStackResource",
        &[
            member("StackName", stack),
            member("LogicalResourceId", logical_id),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: DescribeStackResource({stack}/{logical_id}) must answer 200, got {code} \
         {body}"
    );
    xml_text(&body, "PhysicalResourceId").unwrap_or_default()
}

/// The `DisplayName` Amazon SNS really holds for the topic `arn` names.
async fn topic_display_name(case: &str, router: &MultiServiceRouter, arn: &str) -> String {
    let (code, body) = sns(router, "GetTopicAttributes", &[("TopicArn", arn)]).await;
    assert_eq!(
        code, 200,
        "case {case:?}: GetTopicAttributes({arn}) must answer 200, got {code} {body}"
    );
    xml_blocks(&body, "entry")
        .iter()
        .find(|entry| xml_text(entry, "key").as_deref() == Some("DisplayName"))
        .and_then(|entry| xml_text(entry, "value"))
        .unwrap_or_default()
}

/// The `OutputValue` one stack reports for an output key.
async fn output_value(case: &str, router: &MultiServiceRouter, stack: &str, key: &str) -> String {
    let (code, body) = describe_stack(router, stack).await;
    assert_eq!(
        code, 200,
        "case {case:?}: DescribeStacks({stack}) must answer 200, got {code} {body}"
    );
    xml_texts(&body, "OutputKey")
        .into_iter()
        .zip(xml_texts(&body, "OutputValue"))
        .find(|(output_key, _)| output_key == key)
        .map(|(_, value)| value)
        .unwrap_or_default()
}

/// What the importing stack really holds for the value it imported, on whichever surface the
/// row consumes it.
async fn consumed_value(case: &str, router: &MultiServiceRouter, row: &Row) -> String {
    match row.consumer {
        Consumer::TopicDisplayName { logical_id } => {
            let arn = physical_id(case, router, row.importer, logical_id).await;
            topic_display_name(case, router, &arn).await
        }
        Consumer::StackOutput { key } => output_value(case, router, row.importer, key).await,
    }
}

/// `CreateStack` the template, wait for the create to settle, and assert it completed.
async fn deploy(
    case: &str,
    router: &MultiServiceRouter,
    stack: &str,
    template: &str,
    parameters: &[(&str, &str)],
) {
    let (code, body) = create_stack(router, stack, template, &parameter_members(parameters)).await;
    assert_eq!(
        code, 200,
        "case {case:?}: CreateStack({stack}) must be accepted, got {code} {body}"
    );
    let (settled, reason) = settled_status(case, router, stack).await;
    assert_eq!(
        settled,
        status::CREATE_COMPLETE,
        "case {case:?}: expected {stack} to be created, got {settled:?} ({reason:?})"
    );
}

/// Ask for the revision, and assert the CALL itself was accepted: the model declares no error
/// for an export in use, so what the pin cancels is the stack OPERATION and never the request.
async fn request_the_revision(case: &str, router: &MultiServiceRouter, row: &Row) {
    let mut members = vec![
        member("StackName", row.exporter),
        member("TemplateBody", row.revised_template),
    ];
    members.extend(parameter_members(row.revised_parameters));
    let (code, body) = call(router, "UpdateStack", &members).await;
    assert_eq!(
        code, 200,
        "case {case:?}: UpdateStack({}) must be accepted, got {code} {body}",
        row.exporter
    );
}

#[tokio::test]
async fn an_imported_export_pins_its_publisher_until_the_last_importer_is_gone() {
    for row in rows() {
        let case = row.case;
        let router = router();
        deploy(case, &router, row.exporter, row.template, row.parameters).await;
        deploy(case, &router, row.importer, row.importer_template, &[]).await;

        // The import really reached the importing stack, and the index knows about it: this
        // is the state the pin is derived from.
        let resolved = consumed_value(case, &router, &row).await;
        assert_eq!(
            resolved, row.published,
            "case {case:?}: expected the importer to have resolved {:?}, got {resolved:?}",
            row.published
        );
        let importers = importing_stacks(&router, row.export_name).await;
        assert_eq!(
            importers,
            vec![row.importer.to_string()],
            "case {case:?}: expected the import index to name {}, got {importers:?}",
            row.importer
        );

        // The revision is ACCEPTED and then CANCELLED, naming the export and the importer.
        request_the_revision(case, &router, &row).await;
        let (settled, reason) = settled_status(case, &router, row.exporter).await;
        assert_eq!(
            settled,
            status::UPDATE_ROLLBACK_COMPLETE,
            "case {case:?}: expected the pinned revision to be rolled back, got {settled:?} \
             ({reason:?})"
        );
        let expected = update_canceled_reason(row.export_name, row.importer);
        assert_eq!(
            reason, expected,
            "case {case:?}: expected the reason {expected:?}, got {reason:?}"
        );

        // NOTHING moved: the export still publishes what the importer resolved, and the
        // importer's consuming surface still holds it.
        let still_published = published_value(case, &router, row.export_name).await;
        assert_eq!(
            still_published.as_deref(),
            Some(row.published),
            "case {case:?}: expected {} to go on publishing {:?}, got {still_published:?}",
            row.export_name,
            row.published
        );
        let still_consumed = consumed_value(case, &router, &row).await;
        assert_eq!(
            still_consumed, row.published,
            "case {case:?}: expected the importer to still hold {:?}, got {still_consumed:?}",
            row.published
        );

        // The pin is released with its last holder: the importing stack goes, and the index
        // stops naming it.
        let (code, body) = call(
            &router,
            "DeleteStack",
            &[member("StackName", row.importer)],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: DeleteStack({}) must be accepted, got {code} {body}",
            row.importer
        );
        let deadline = Instant::now() + DEADLINE;
        loop {
            let holders = importing_stacks(&router, row.export_name).await;
            if !holders.contains(&row.importer.to_string()) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: expected the deleted importer to drop out of the index for {}, \
                 still {holders:?}",
                row.export_name
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }

        // ...and the byte-identical revision now goes through.
        request_the_revision(case, &router, &row).await;
        let (settled, reason) = settled_status(case, &router, row.exporter).await;
        assert_eq!(
            settled,
            status::UPDATE_COMPLETE,
            "case {case:?}: expected the released revision to complete, got {settled:?} \
             ({reason:?})"
        );
        let published_now = published_value(case, &router, row.export_name).await;
        assert_eq!(
            published_now.as_deref(),
            row.revised_publication,
            "case {case:?}: expected {} to publish {:?} once the pin was released, got \
             {published_now:?}",
            row.export_name,
            row.revised_publication
        );
    }
}
