// SPDX-License-Identifier: CC0-1.0

//! What a stack publishes about a CloudFormation HOOK it declares.
//!
//! The rule under test: **a declared resource publishes the return values its type
//! documents, so every `Outputs` entry of a completed stack carries its resolved
//! `OutputValue`; for the two Hook types a template declares a Hook with
//! (`AWS::CloudFormation::GuardHook` and `AWS::CloudFormation::LambdaHook`) those values are
//! the Hook ARN — what `Ref` returns — and `HookArn`, the ARN of the activated Hook, which
//! is the registry ARN this account and Region holds that Hook under.**
//!
//! Every row drives the emulator through its own wire: `CreateStack` with the declaration
//! and `DescribeStacks` for what the stack publishes, so what is asserted is what a client
//! reading the outputs sees.
//!
//! The rows differ along what the rule generalises over: which Hook type is declared, and
//! whether the declaration states the `Alias` the Hook's own type name is (so the ARN is
//! built on the alias) or states none (so it is built on the name CloudFormation generated
//! for the resource).

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::harness::{
    call, create_stack, member, router, wait_for_the_transition, xml_text, xml_texts,
};

/// One row: the Hook type declared, and the `Alias` the declaration states.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    resource_type: &'static str,
    alias: Option<&'static str>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a Guard Hook declaring its alias",
            stack_name: "hook-guard-aliased",
            resource_type: "AWS::CloudFormation::GuardHook",
            alias: Some("Private::Guard::QueueEncryption"),
        },
        Row {
            case: "a Lambda Hook declaring its alias",
            stack_name: "hook-lambda-aliased",
            resource_type: "AWS::CloudFormation::LambdaHook",
            alias: Some("Private::Lambda::QueueEncryption"),
        },
        Row {
            case: "a Guard Hook declaring no alias at all",
            stack_name: "hook-guard-unaliased",
            resource_type: "AWS::CloudFormation::GuardHook",
            alias: None,
        },
    ]
}

/// The template one row deploys: the Hook, and outputs publishing both values its type
/// documents.
fn template(resource_type: &str, alias: Option<&str>) -> String {
    let mut properties = serde_json::Map::new();
    if let Some(alias) = alias {
        properties.insert("Alias".to_string(), Value::String(alias.to_string()));
    }
    properties.insert(
        "ExecutionRole".to_string(),
        Value::String("arn:aws:iam::000000000000:role/hook-execution".to_string()),
    );
    properties.insert("FailureMode".to_string(), Value::String("FAIL".to_string()));
    properties.insert(
        "HookStatus".to_string(),
        Value::String("ENABLED".to_string()),
    );
    properties.insert(
        "TargetOperations".to_string(),
        json!(["RESOURCE", "STACK"]),
    );
    json!({
        "Resources": {
            "Hook": {
                "Type": resource_type,
                "Properties": Value::Object(properties),
            },
        },
        "Outputs": {
            "HookRef": {"Value": {"Ref": "Hook"}},
            "HookArn": {"Value": {"Fn::GetAtt": ["Hook", "HookArn"]}},
        },
    })
    .to_string()
}

/// A stack's reported outputs, as the key/value map a client reads — built from the two
/// members in PARALLEL, so an entry carrying a key and no value is missing from it and the
/// assertion names what was absent.
fn outputs_of(xml: &str) -> BTreeMap<String, String> {
    xml_texts(xml, "OutputKey")
        .into_iter()
        .zip(xml_texts(xml, "OutputValue"))
        .collect()
}

/// The ARN form the registry publishes for an extension of this account and Region:
/// `arn:aws:cloudformation:<region>:<account>:type/hook/<Type-Name>`, with each `::` of the
/// type name written as one `-`.
///
/// Spelled here from the published form rather than read from the crate, so the suite
/// asserts the shape a client sees.
fn hook_arn(type_name: &str) -> String {
    format!(
        "arn:aws:cloudformation:{region}:{account}:type/hook/{name}",
        region = super::harness::REGION,
        account = "000000000000",
        name = type_name.replace("::", "-"),
    )
}

#[tokio::test]
async fn a_declared_hook_publishes_its_arn_in_the_stacks_outputs() {
    let router = router();
    for row in rows() {
        let case = row.case;
        let body = template(row.resource_type, row.alias);
        let (status_code, created) = create_stack(&router, row.stack_name, &body, &[]).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected CreateStack to be accepted, got {status_code} with {created}"
        );
        wait_for_the_transition().await;
        let (status_code, described) =
            call(&router, "DescribeStacks", &[member("StackName", row.stack_name)]).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected DescribeStacks to answer, got {status_code} with {described}"
        );
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to complete, got {described}"
        );
        let outputs = outputs_of(&described);
        // Every declared output carries a VALUE: a key with no value is absent from the map
        // a client builds, which is the defect this row holds the stack to.
        assert_eq!(
            outputs.len(),
            2,
            "case {case:?}: expected both declared outputs to carry a value, got {outputs:?} \
             from {described}"
        );
        let published = match row.alias {
            Some(alias) => hook_arn(alias),
            // A declaration that states no alias leaves the ARN on the name CloudFormation
            // generated for the resource, which is not a value a suite can spell — so the
            // row asserts the FORM instead.
            None => outputs.get("HookArn").cloned().unwrap_or_default(),
        };
        assert_eq!(
            outputs.get("HookArn"),
            Some(&published),
            "case {case:?}: expected HookArn to be the registry ARN of the Hook, got {outputs:?}"
        );
        assert_eq!(
            outputs.get("HookRef"),
            Some(&published),
            "case {case:?}: Ref returns the Hook ARN, got {outputs:?}"
        );
        let prefix = format!(
            "arn:aws:cloudformation:{}:000000000000:type/hook/",
            super::harness::REGION
        );
        assert!(
            published.starts_with(&prefix) && published.len() > prefix.len(),
            "case {case:?}: expected a registry Hook ARN of this Region, got {published:?}"
        );
        assert!(
            !published.contains("::"),
            "case {case:?}: an extension ARN writes each :: of the type name as one -, got \
             {published:?}"
        );
    }
}
