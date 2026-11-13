// SPDX-License-Identifier: CC0-1.0

//! Replay two unmodified, committed AWS CDK templates, then read their OWNING services.
//! A CloudFormation catalog record is not proof of a deployed resource. These tests use
//! the existing in-process harness: no CLI, socket, asset publishing or bootstrap claim.
//! Fixture provenance and Apache-2.0 notices live beside the included JSON files.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::change_set_lifecycle::{create_creating_change_set, describe_change_set, execute_change_set};
use crate::dynamic_references::put_parameter;
use crate::stack_lifecycle::{
    call, describe_stack, member, router, sns, wait_for_the_transition, xml_blocks, xml_text, REGION,
};

type Check<T = ()> = Result<T, String>;
const PHASE_LIMIT: Duration = Duration::from_secs(30);
const MAX_POLLS: usize = 40;
const SNS_TEMPLATE: &str = include_str!(
    "../fixtures/cdk/79914032/integ.sns-fifo/SNSFifoInteg.template.json"
);
const LOGS_TEMPLATE: &str = include_str!(
    "../fixtures/cdk/79914032/integ.expose-metric/aws-cdk-expose-metric-integ.template.json"
);
const TOPIC: &str = "MyTopic86869434";
const GROUP: &str = "LogGroupF5B46931";
const FILTER: &str = "MetricFilter1B93B6E5";
const ALARM: &str = "alarmfrommetricfilterF546D67D";

#[derive(Default)]
struct Deployed {
    stack_id: Option<String>,
    ids: BTreeMap<String, String>,
}

fn require(ok: bool, message: impl Into<String>) -> Check {
    if ok { Ok(()) } else { Err(message.into()) }
}

fn accepted(operation: &str, response: (u16, String)) -> Check<String> {
    let (status, body) = response;
    require(status == 200, format!("{operation}: HTTP {status}: {body}"))?;
    Ok(body)
}

fn id<'a>(deployed: &'a Deployed, logical: &str) -> Check<&'a str> {
    deployed.ids.get(logical).map(String::as_str)
        .ok_or_else(|| format!("missing CFN identity for {logical}"))
}

// The CloudWatch model uses awsJson1_0, not the historical query protocol. Logs uses
// awsJson1_1. Mirror their existing service harnesses; no live signing credentials.
async fn json_call(
    router: &MultiServiceRouter,
    service: &str,
    operation: &str,
    body: &Value,
) -> Check<Value> {
    let (prefix, version) = match service {
        "monitoring" => ("GraniteServiceVersion20100801", "1.0"),
        "logs" => ("Logs_20140328", "1.1"),
        _ => return Err(format!("unregistered test service {service}")),
    };
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{service}.{REGION}.amazonaws.com/"))
        .header("content-type", format!("application/x-amz-json-{version}"))
        .header("x-amz-target", format!("{prefix}.{operation}"))
        .header("authorization", format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
             SignedHeaders=host, Signature=0123456789abcdef"
        ))
        .body(bytes::Bytes::from(body.to_string()))
        .map_err(|error| error.to_string())?;
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    require(status == 200, format!(
        "{service}.{operation}: HTTP {status}: {}", String::from_utf8_lossy(&raw)
    ))?;
    if raw.is_empty() { return Ok(Value::Null); }
    serde_json::from_slice(&raw).map_err(|error| format!("{service}.{operation}: {error}"))
}

fn array<'a>(document: &'a Value, key: &str) -> Check<&'a Vec<Value>> {
    document.get(key).and_then(Value::as_array)
        .ok_or_else(|| format!("missing array {key}: {document}"))
}

fn equal_fields(actual: &Value, expected: &Value) -> Check {
    let fields = expected.as_object().ok_or("expected fields must be an object")?;
    for (key, wanted) in fields {
        let got = actual.get(key).ok_or_else(|| format!("owner omitted field {key}: {actual}"))?;
        let matches = if wanted.is_number() {
            got.as_f64() == wanted.as_f64()
        } else {
            got == wanted
        };
        require(matches, format!("owner field {key}: wanted {wanted}, got {got}"))?;
    }
    Ok(())
}

async fn wait_stack(router: &MultiServiceRouter, stack: &str, terminal: &str) -> Check {
    for _ in 0..MAX_POLLS {
        let body = accepted("DescribeStacks", describe_stack(router, stack).await)?;
        let status = xml_text(&body, "StackStatus").ok_or("missing StackStatus")?;
        if status == terminal { return Ok(()); }
        require(status.ends_with("_IN_PROGRESS"), format!(
            "deployment_failed: wanted {terminal}, got {status}: {body}"
        ))?;
        wait_for_the_transition().await;
    }
    Err(format!("timeout waiting for {terminal}"))
}

async fn deploy(
    router: &MultiServiceRouter,
    stack: &str,
    template: &str,
    logical_ids: &[&str],
    deployed: &mut Deployed,
) -> Check {
    let body = accepted("CreateChangeSet", create_creating_change_set(
        router, stack, "snapshot-owner-effects", template,
    ).await)?;
    deployed.stack_id = xml_text(&body, "StackId");
    let stack_id = deployed.stack_id.as_deref().ok_or("CreateChangeSet omitted StackId")?;
    let change_set = xml_text(&body, "Id").ok_or("CreateChangeSet omitted Id")?;
    let mut ready = false;
    for _ in 0..MAX_POLLS {
        let body = accepted("DescribeChangeSet", describe_change_set(
            router, &change_set, Some(stack_id),
        ).await)?;
        let status = xml_text(&body, "Status").ok_or("missing change-set Status")?;
        if status == "CREATE_COMPLETE" { ready = true; break; }
        require(status == "CREATE_PENDING" || status == "CREATE_IN_PROGRESS", format!(
            "change_set_failed: {status}: {body}"
        ))?;
        wait_for_the_transition().await;
    }
    require(ready, "timeout waiting for change set")?;
    accepted("ExecuteChangeSet", execute_change_set(router, &change_set, Some(stack_id)).await)?;
    wait_stack(router, stack_id, "CREATE_COMPLETE").await?;
    for logical in logical_ids {
        let body = accepted("DescribeStackResource", call(router, "DescribeStackResource", &[
            member("StackName", stack_id), member("LogicalResourceId", logical),
        ]).await)?;
        let physical = xml_text(&body, "PhysicalResourceId").filter(|id| !id.is_empty())
            .ok_or_else(|| format!("missing physical identity for {logical}: {body}"))?;
        deployed.ids.insert((*logical).to_string(), physical);
    }
    Ok(())
}

async fn check_topic(router: &MultiServiceRouter, deployed: &Deployed, present: bool) -> Check {
    let arn = id(deployed, TOPIC)?;
    let (status, body) = sns(router, "GetTopicAttributes", &[("TopicArn", arn)]).await;
    if !present {
        return require(status == 404 && xml_text(&body, "Code").as_deref() == Some("NotFound"),
            format!("topic still exists or read failed after deletion: HTTP {status}: {body}"));
    }
    accepted("GetTopicAttributes", (status, body.clone()))?;
    let attributes: BTreeMap<String, String> = xml_blocks(&body, "entry").iter()
        .filter_map(|entry| Some((xml_text(entry, "key")?, xml_text(entry, "value")?)))
        .collect();
    for (key, value) in [("TopicArn", arn), ("FifoTopic", "true"),
        ("ContentBasedDeduplication", "true"), ("DisplayName", "fooDisplayName")] {
        require(attributes.get(key).map(String::as_str) == Some(value),
            format!("topic {arn}: expected {key}={value}, got {attributes:?}"))?;
    }
    Ok(())
}

async fn alarms(router: &MultiServiceRouter, name: &str) -> Check<Vec<Value>> {
    let response = json_call(router, "monitoring", "DescribeAlarms", &json!({
        "AlarmNames": [name], "AlarmTypes": ["MetricAlarm"],
    })).await?;
    Ok(array(&response, "MetricAlarms")?.clone())
}

async fn check_logs(router: &MultiServiceRouter, deployed: &Deployed, present: bool) -> Check {
    let group = id(deployed, GROUP)?;
    let response = json_call(router, "logs", "DescribeLogGroups", &json!({
        "logGroupNamePrefix": group,
    })).await?;
    let found: Vec<&Value> = array(&response, "logGroups")?.iter()
        .filter(|entry| entry["logGroupName"].as_str() == Some(group)).collect();
    if present {
        require(found.len() == 1, format!("owner_absent: log group {group}: {response}"))?;
        equal_fields(found[0], &json!({"retentionInDays": 731}))?;
        let filter = id(deployed, FILTER)?;
        let response = json_call(router, "logs", "DescribeMetricFilters", &json!({
            "logGroupName": group, "filterNamePrefix": filter,
        })).await?;
        let filters: Vec<&Value> = array(&response, "metricFilters")?.iter()
            .filter(|entry| entry["filterName"].as_str() == Some(filter)).collect();
        require(filters.len() == 1, format!("owner_absent: metric filter {filter}: {response}"))?;
        equal_fields(filters[0], &json!({"filterPattern": "{ $.latency = \"*\" }"}))?;
        let transformations = array(filters[0], "metricTransformations")?;
        require(transformations.len() == 1, "expected exactly one metric transformation")?;
        equal_fields(&transformations[0], &json!({
            "metricNamespace": "MyApp", "metricName": "Latency", "metricValue": "$.latency",
        }))?;
    } else {
        require(found.is_empty(), format!("log group remains after deletion: {response}"))?;
        // DescribeMetricFilters may search all groups; the filter's exact name must be gone.
        let response = json_call(router, "logs", "DescribeMetricFilters", &json!({
            "filterNamePrefix": id(deployed, FILTER)?,
        })).await?;
        require(array(&response, "metricFilters")?.iter().all(|entry|
            entry["filterName"].as_str() != deployed.ids.get(FILTER).map(String::as_str)),
            format!("metric filter remains after deletion: {response}"))?;
    }
    let alarm = id(deployed, ALARM)?;
    let held = alarms(router, alarm).await?;
    if !present {
        return require(held.is_empty(), format!("alarm remains after deletion: {held:?}"));
    }
    require(held.len() == 1, format!(
        "CREATE_COMPLETE/owner_absent: expected one CloudWatch alarm {alarm}, got {held:?}"
    ))?;
    equal_fields(&held[0], &json!({"AlarmName": alarm}))?;
    let template: Value = serde_json::from_str(LOGS_TEMPLATE).map_err(|error| error.to_string())?;
    equal_fields(&held[0], &template["Resources"][ALARM]["Properties"])
}

// Calibrate the missing-alarm oracle on a DIFFERENT engine, never by creating the
// snapshot's missing alarm. A read/parser failure is not evidence of an absent owner.
async fn alarm_oracle() -> Check {
    let router = router();
    let name = "cdk-snapshot-oracle-control";
    let template: Value = serde_json::from_str(LOGS_TEMPLATE).map_err(|error| error.to_string())?;
    let mut properties = template["Resources"][ALARM]["Properties"].clone();
    properties["AlarmName"] = json!(name);
    json_call(&router, "monitoring", "PutMetricAlarm", &properties).await?;
    let checked = async {
        let held = alarms(&router, name).await?;
        require(held.len() == 1, format!("alarm oracle: expected one record, got {held:?}"))?;
        equal_fields(&held[0], &properties)
    }.await;
    let deleted = json_call(&router, "monitoring", "DeleteAlarms", &json!({"AlarmNames": [name]})).await;
    let gone = alarms(&router, name).await?;
    require(gone.is_empty(), "oracle control alarm remains after DeleteAlarms")?;
    deleted?;
    checked
}

async fn cleanup(router: &MultiServiceRouter, deployed: &Deployed, logs: bool) -> Check {
    let Some(stack_id) = &deployed.stack_id else { return Ok(()); };
    accepted("DeleteStack", call(router, "DeleteStack", &[member("StackName", stack_id)]).await)?;
    wait_stack(router, stack_id, "DELETE_COMPLETE").await?;
    // Partial deployment failure may leave no published identities. Its original failure
    // still fails the test; never call that incomplete owner evidence a success.
    let expected_ids = if logs { 3 } else { 1 };
    if deployed.ids.len() != expected_ids { return Ok(()); }
    let mut last_error = String::from("owner deletion was not observed");
    for _ in 0..MAX_POLLS {
        let check = if logs { check_logs(router, deployed, false).await }
            else { check_topic(router, deployed, false).await };
        match check {
            Ok(()) => return Ok(()),
            Err(error) => last_error = error,
        }
        wait_for_the_transition().await;
    }
    Err(format!("owner deletion not observed: {last_error}"))
}

async fn run_snapshot(logs: bool) {
    if logs {
        let oracle = tokio::time::timeout(PHASE_LIMIT, alarm_oracle()).await;
        assert!(matches!(&oracle, Ok(Ok(()))), "CloudWatch oracle failed: {oracle:?}");
    }
    let router = router();
    // Explicit fixture precondition, NOT a claim that cdk bootstrap was run.
    tokio::time::timeout(PHASE_LIMIT, put_parameter(
        &router, "committed CDK snapshot", "/cdk-bootstrap/hnb659fds/version", "String", "21",
    )).await.expect("bootstrap-version fixture setup timed out");
    let mut deployed = Deployed::default();
    let (stack, template, logical_ids): (&str, &str, &[&str]) = if logs {
        ("cdk-snapshot-logs", LOGS_TEMPLATE, &[GROUP, FILTER, ALARM])
    } else {
        ("cdk-snapshot-sns", SNS_TEMPLATE, &[TOPIC])
    };
    let result = tokio::time::timeout(PHASE_LIMIT, async {
        deploy(&router, stack, template, logical_ids, &mut deployed).await?;
        if logs { check_logs(&router, &deployed, true).await }
            else { check_topic(&router, &deployed, true).await }
    }).await;
    // Always attempt stack cleanup before asserting the owner witness, including on RED.
    let cleanup = tokio::time::timeout(PHASE_LIMIT, cleanup(&router, &deployed, logs)).await;
    assert!(matches!(&result, Ok(Ok(()))) && matches!(&cleanup, Ok(Ok(()))),
        "snapshot={stack}; owner result={result:?}; cleanup result={cleanup:?}");
}

#[tokio::test]
async fn committed_sns_fifo_snapshot_materialises_its_topic() {
    run_snapshot(false).await;
}

#[tokio::test]
async fn committed_logs_metric_snapshot_materialises_its_alarm() {
    run_snapshot(true).await;
}
