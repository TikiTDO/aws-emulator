// SPDX-License-Identifier: CC0-1.0

//! What `GetTemplateSummary`'s `ResourceTypes` reports about a template's resources, end to
//! end over the awsQuery wire.
//!
//! THE RULE: "a list of all the template resource types that are defined in the template" is
//! the SET of types the body declares — each type once, in ascending order — whatever order
//! the body declares resources of them in and however many resources share a type. The
//! per-RESOURCE reading of the same section is `ResourceIdentifierSummaries`, which carries
//! the logical ids of each type in the order the body DECLARES them, so the two members are
//! ordered by different things and this suite asserts both of one response.
//!
//! The rows differ along what the rule generalises over: how many resources share a type,
//! whether the declared order agrees with the ascending one, how many types the body
//! declares, and the spelling of the body (JSON and YAML).

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{call, list_members, member, router, xml_blocks, xml_text, xml_texts};

/// `GetTemplateSummary` over a body the request carries.
async fn summarise(router: &MultiServiceRouter, template: &str) -> (u16, String) {
    call(
        router,
        "GetTemplateSummary",
        &[member("TemplateBody", template)],
    )
    .await
}

/// The `ResourceTypes` the response states, in wire order.
fn reported_types(xml: &str) -> Vec<String> {
    xml_blocks(xml, "ResourceTypes")
        .first()
        .map(|block| xml_texts(block, "member"))
        .unwrap_or_default()
}

/// Each `ResourceIdentifierSummary` as `(ResourceType, LogicalResourceIds)`, in wire order.
fn reported_summaries(xml: &str) -> Vec<(String, Vec<String>)> {
    list_members(xml, "ResourceIdentifierSummaries")
        .iter()
        .map(|entry| {
            let ids = xml_blocks(entry, "LogicalResourceIds")
                .first()
                .map(|block| xml_texts(block, "member"))
                .unwrap_or_default();
            (xml_text(entry, "ResourceType").unwrap_or_default(), ids)
        })
        .collect()
}

/// One row: a body, the types its summary must report, and the identifier summaries beside
/// them.
struct Row {
    case: &'static str,
    template: String,
    types: Vec<&'static str>,
    summaries: Vec<(&'static str, Vec<&'static str>)>,
}

fn rows() -> Vec<Row> {
    // The bodies are written out as TEXT rather than built with `json!`, which sorts an
    // object's members as it serialises them: what these rows turn on is the order the body
    // itself states, so each row states it.
    vec![
        Row {
            case: "two types declared against the ascending order",
            template: r#"{"Resources":{
                "Store":{"Type":"AWS::S3::Bucket"},
                "Access":{"Type":"AWS::IAM::Role"}}}"#
                .to_string(),
            types: vec!["AWS::IAM::Role", "AWS::S3::Bucket"],
            summaries: vec![
                ("AWS::S3::Bucket", vec!["Store"]),
                ("AWS::IAM::Role", vec!["Access"]),
            ],
        },
        Row {
            case: "one type declared by three resources",
            template: r#"{"Resources":{
                "Third":{"Type":"AWS::SNS::Topic"},
                "First":{"Type":"AWS::SNS::Topic"},
                "Second":{"Type":"AWS::SNS::Topic"}}}"#
                .to_string(),
            types: vec!["AWS::SNS::Topic"],
            summaries: vec![("AWS::SNS::Topic", vec!["Third", "First", "Second"])],
        },
        Row {
            case: "a body whose declared order already is the ascending one",
            template: r#"{"Resources":{
                "Access":{"Type":"AWS::IAM::Role"},
                "Store":{"Type":"AWS::S3::Bucket"}}}"#
                .to_string(),
            types: vec!["AWS::IAM::Role", "AWS::S3::Bucket"],
            summaries: vec![
                ("AWS::IAM::Role", vec!["Access"]),
                ("AWS::S3::Bucket", vec!["Store"]),
            ],
        },
        Row {
            case: "YAML, four resources of three types, two of them sharing one",
            template: "Resources:\n  \
                 Zebra:\n    Type: AWS::SQS::Queue\n  \
                 Alpha:\n    Type: AWS::SNS::Topic\n  \
                 Mid:\n    Type: AWS::SQS::Queue\n  \
                 Store:\n    Type: AWS::S3::Bucket\n"
                .to_string(),
            types: vec!["AWS::S3::Bucket", "AWS::SNS::Topic", "AWS::SQS::Queue"],
            summaries: vec![
                ("AWS::SQS::Queue", vec!["Zebra", "Mid"]),
                ("AWS::SNS::Topic", vec!["Alpha"]),
                ("AWS::S3::Bucket", vec!["Store"]),
            ],
        },
        Row {
            case: "a single resource, where the set and the declaration agree trivially",
            template: json!({"Resources": {"Only": {"Type": "AWS::SNS::Topic"}}}).to_string(),
            types: vec!["AWS::SNS::Topic"],
            summaries: vec![("AWS::SNS::Topic", vec!["Only"])],
        },
    ]
}

#[tokio::test]
async fn a_summary_reports_each_declared_type_once_in_ascending_order() {
    let router = router();
    for row in rows() {
        let case = row.case;
        let (status, body) = summarise(&router, &row.template).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the body summarised, got {status} {body}"
        );

        let reported = reported_types(&body);
        assert_eq!(
            reported, row.types,
            "case {case:?}: expected the declared types once each, ascending, {:?}, got \
             {reported:?}",
            row.types
        );

        // The same response's per-resource reading: the identifier summaries follow the
        // order the BODY declares its resources in, which is what shows the ordering above
        // is this member's own rather than the reader's.
        let expected: Vec<(String, Vec<String>)> = row
            .summaries
            .iter()
            .map(|(resource_type, ids)| {
                (
                    (*resource_type).to_string(),
                    ids.iter().map(|id| (*id).to_string()).collect(),
                )
            })
            .collect();
        let summaries = reported_summaries(&body);
        assert_eq!(
            summaries, expected,
            "case {case:?}: expected the identifier summaries {expected:?}, got {summaries:?}"
        );
    }
}
