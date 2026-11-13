// SPDX-License-Identifier: CC0-1.0

//! `AWS::CloudWatch::Alarm`, end to end: a stack that declares an alarm leaves CloudWatch
//! holding it, and deleting the stack takes it away again.
//!
//! RULE: CloudFormation creates the alarm through CloudWatch's own `PutMetricAlarm`, so the
//! alarm is CloudWatch's resource, read back through `DescribeAlarms` rather than through
//! anything this service keeps. Rook's Foundation census found ten of these reported
//! `CREATE_COMPLETE` while CloudWatch held none; this is the check that the report and the
//! resource now agree. The shape is Karma's: one metric, a dimension, a named alarm.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, router, wait_for_the_transition, xml_text,
    REGION,
};

/// Call one CloudWatch operation through the same router, `awsJson1_0` the way an SDK sends
/// it: the status and the parsed answer document.
async fn cloudwatch(router: &MultiServiceRouter, operation: &str, request: &Value) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri(format!("https://monitoring.{REGION}.amazonaws.com/"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("GraniteServiceVersion20100801.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/monitoring/\
                 aws4_request, SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
            ),
        )
        .body(bytes::Bytes::from(request.to_string()))
        .expect("a well-formed CloudWatch request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let body = response.into_body().to_vec();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

/// The metric alarm CloudWatch holds under `name`, or `None`.
async fn described_alarm(router: &MultiServiceRouter, name: &str) -> Option<Value> {
    let (status, document) = cloudwatch(
        router,
        "DescribeAlarms",
        &json!({"AlarmNames": [name], "AlarmTypes": ["MetricAlarm"]}),
    )
    .await;
    assert_eq!(status, 200, "DescribeAlarms must answer, got {status} {document}");
    document
        .get("MetricAlarms")
        .and_then(Value::as_array)
        .and_then(|alarms| {
            alarms
                .iter()
                .find(|alarm| alarm.get("AlarmName").and_then(Value::as_str) == Some(name))
                .cloned()
        })
}

#[tokio::test]
async fn a_stack_alarm_is_held_by_cloudwatch_and_goes_with_the_stack() {
    let router = router();
    let stack = "cloudwatch-alarm-stack";
    let name = "queue-depth-high";
    let template = json!({
        "Resources": {
            "QueueDepth": {"Type": "AWS::CloudWatch::Alarm", "Properties": {
                "AlarmName": name,
                "AlarmDescription": "messages waiting",
                "Namespace": "AWS/SQS",
                "MetricName": "ApproximateNumberOfMessagesVisible",
                "Dimensions": [{"Name": "QueueName", "Value": "jobs"}],
                "Statistic": "Maximum",
                "Period": 300,
                "EvaluationPeriods": 2,
                "Threshold": 100,
                "ComparisonOperator": "GreaterThanThreshold",
                "TreatMissingData": "notBreaching"
            }}
        },
        "Outputs": {
            "AlarmRef": {"Value": {"Ref": "QueueDepth"}},
            "AlarmArn": {"Value": {"Fn::GetAtt": ["QueueDepth", "Arn"]}}
        }
    })
    .to_string();

    let (status, body) = create_stack(&router, stack, &template, &[]).await;
    assert_eq!(status, 200, "CreateStack must answer, got {status} {body}");
    wait_for_the_transition().await;
    let (status, body) = describe_stack(&router, stack).await;
    assert_eq!(status, 200, "DescribeStacks must answer, got {status} {body}");
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "the stack must complete, got {body}"
    );

    let alarm = described_alarm(&router, name)
        .await
        .expect("CloudWatch must hold the alarm the stack reports as created");
    assert_eq!(alarm["Namespace"], "AWS/SQS");
    assert_eq!(alarm["MetricName"], "ApproximateNumberOfMessagesVisible");
    assert_eq!(alarm["ComparisonOperator"], "GreaterThanThreshold");
    assert_eq!(alarm["Threshold"].as_f64(), Some(100.0));
    assert_eq!(alarm["EvaluationPeriods"].as_i64(), Some(2));
    assert_eq!(alarm["Dimensions"][0]["Value"], "jobs");
    assert!(
        body.contains(&format!("<OutputValue>{name}</OutputValue>")),
        "Ref must be the alarm's name, got {body}"
    );
    let arn = alarm["AlarmArn"].as_str().expect("the alarm carries its ARN");
    assert!(
        body.contains(&format!("<OutputValue>{arn}</OutputValue>")),
        "GetAtt Arn must be CloudWatch's ARN {arn}, got {body}"
    );

    let (status, body) = call(&router, "DeleteStack", &[member("StackName", stack)]).await;
    assert_eq!(status, 200, "DeleteStack must answer, got {status} {body}");
    wait_for_the_transition().await;
    assert!(
        described_alarm(&router, name).await.is_none(),
        "deleting the stack must delete the alarm"
    );
}
